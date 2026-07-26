#![forbid(unsafe_code)]

use tea_core::{ApprovalPolicy, RiskLevel, TicketSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    RequestApproval { reason: String },
    Deny { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyInput {
    pub source: TicketSource,
    pub risk_level: RiskLevel,
    pub approval_policy: ApprovalPolicy,
    pub has_approval: bool,
    pub has_evidence: bool,
}

pub fn evaluate_run(input: &PolicyInput) -> PolicyDecision {
    if input.risk_level == RiskLevel::High && !input.has_approval {
        return PolicyDecision::RequestApproval {
            reason: "high risk ticket requires approval".to_string(),
        };
    }

    match input.approval_policy {
        ApprovalPolicy::PlanOnly if !input.has_approval => PolicyDecision::RequestApproval {
            reason: "plan-only ticket requires explicit approval before run".to_string(),
        },
        ApprovalPolicy::HumanBeforeExecute if !input.has_approval => {
            PolicyDecision::RequestApproval {
                reason: "human approval required before execute".to_string(),
            }
        }
        ApprovalPolicy::ManualOnly => PolicyDecision::Deny {
            reason: "manual-only ticket cannot run automatically".to_string(),
        },
        ApprovalPolicy::AlwaysAuto => {
            if input.risk_level == RiskLevel::High {
                PolicyDecision::RequestApproval {
                    reason: "high risk ticket cannot always-auto without approval".to_string(),
                }
            } else {
                PolicyDecision::Allow
            }
        }
        _ => {
            if input.has_approval
                || matches!(
                    input.approval_policy,
                    ApprovalPolicy::AutoIfLowRisk | ApprovalPolicy::AutoIfValidationPasses
                )
            {
                PolicyDecision::Allow
            } else {
                PolicyDecision::RequestApproval {
                    reason: "approval policy requires human decision".to_string(),
                }
            }
        }
    }
}

pub fn evaluate_close(input: &PolicyInput) -> PolicyDecision {
    if !input.has_evidence {
        PolicyDecision::Deny {
            reason: "ticket close requires evidence".to_string(),
        }
    } else if matches!(input.approval_policy, ApprovalPolicy::HumanBeforeCompletion)
        && !input.has_approval
    {
        PolicyDecision::RequestApproval {
            reason: "human approval required before completion".to_string(),
        }
    } else {
        PolicyDecision::Allow
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_plan_only_requests_approval_before_run() {
        let decision = evaluate_run(&PolicyInput {
            source: TicketSource::Hook,
            risk_level: RiskLevel::Medium,
            approval_policy: ApprovalPolicy::PlanOnly,
            has_approval: false,
            has_evidence: false,
        });
        assert!(matches!(decision, PolicyDecision::RequestApproval { .. }));
    }

    #[test]
    fn plan_only_allows_run_after_explicit_approval() {
        let decision = evaluate_run(&PolicyInput {
            source: TicketSource::Hook,
            risk_level: RiskLevel::Medium,
            approval_policy: ApprovalPolicy::PlanOnly,
            has_approval: true,
            has_evidence: false,
        });
        assert_eq!(decision, PolicyDecision::Allow);
    }

    #[test]
    fn manual_only_denies_run() {
        let decision = evaluate_run(&PolicyInput {
            source: TicketSource::Human,
            risk_level: RiskLevel::Low,
            approval_policy: ApprovalPolicy::ManualOnly,
            has_approval: true,
            has_evidence: false,
        });
        assert!(matches!(decision, PolicyDecision::Deny { .. }));
    }

    #[test]
    fn close_without_evidence_is_denied() {
        let decision = evaluate_close(&PolicyInput {
            source: TicketSource::Human,
            risk_level: RiskLevel::Low,
            approval_policy: ApprovalPolicy::AlwaysAuto,
            has_approval: true,
            has_evidence: false,
        });
        assert!(matches!(decision, PolicyDecision::Deny { .. }));
    }
}
