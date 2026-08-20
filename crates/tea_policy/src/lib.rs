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
    /// Tea does not currently persist a pre-execution validation result, so API
    /// callers must pass `false` until such evidence exists. Keeping this
    /// explicit prevents `AutoIfValidationPasses` from becoming unconditional.
    pub validation_passed: bool,
}

pub fn evaluate_run(input: &PolicyInput) -> PolicyDecision {
    if input.risk_level == RiskLevel::High && !input.has_approval {
        return PolicyDecision::RequestApproval {
            reason: "high risk ticket requires approval".to_string(),
        };
    }

    match input.approval_policy {
        ApprovalPolicy::ManualOnly => PolicyDecision::Deny {
            reason: "manual-only ticket cannot run automatically".to_string(),
        },
        ApprovalPolicy::AlwaysAuto | ApprovalPolicy::HumanBeforeCompletion => PolicyDecision::Allow,
        ApprovalPolicy::AutoIfLowRisk => {
            if input.has_approval || input.risk_level == RiskLevel::Low {
                PolicyDecision::Allow
            } else {
                PolicyDecision::RequestApproval {
                    reason: "auto-if-low-risk requires a low-risk ticket or human approval"
                        .to_string(),
                }
            }
        }
        ApprovalPolicy::AutoIfValidationPasses => {
            if input.has_approval || input.validation_passed {
                PolicyDecision::Allow
            } else {
                PolicyDecision::RequestApproval {
                    reason: "auto-if-validation-passes requires a successful validation result or human approval"
                        .to_string(),
                }
            }
        }
        ApprovalPolicy::PlanOnly
        | ApprovalPolicy::HumanBeforeExecute
        | ApprovalPolicy::HumanBeforeWrite
        | ApprovalPolicy::HumanBeforeExternalNetwork
        | ApprovalPolicy::HumanBeforeDestructiveAction => {
            if input.has_approval {
                PolicyDecision::Allow
            } else {
                PolicyDecision::RequestApproval {
                    reason: "approval policy requires human decision before run".to_string(),
                }
            }
        }
    }
}

/// Relative strength of the policy's pre-execution gate. Completion-only
/// approval is intentionally not counted here; `evaluate_close` enforces it.
pub fn run_gate_strength(policy: ApprovalPolicy) -> u8 {
    match policy {
        ApprovalPolicy::AlwaysAuto | ApprovalPolicy::HumanBeforeCompletion => 0,
        ApprovalPolicy::AutoIfLowRisk => 1,
        ApprovalPolicy::AutoIfValidationPasses => 2,
        ApprovalPolicy::PlanOnly
        | ApprovalPolicy::HumanBeforeExecute
        | ApprovalPolicy::HumanBeforeWrite
        | ApprovalPolicy::HumanBeforeExternalNetwork
        | ApprovalPolicy::HumanBeforeDestructiveAction => 3,
        ApprovalPolicy::ManualOnly => 4,
    }
}

pub fn weakens_run_gate(current: ApprovalPolicy, proposed: ApprovalPolicy) -> bool {
    run_gate_strength(proposed) < run_gate_strength(current)
}

pub fn weakens_approval_policy(current: ApprovalPolicy, proposed: ApprovalPolicy) -> bool {
    weakens_run_gate(current, proposed)
        || (current == ApprovalPolicy::HumanBeforeCompletion
            && proposed != ApprovalPolicy::HumanBeforeCompletion)
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
            validation_passed: false,
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
            validation_passed: false,
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
            validation_passed: false,
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
            validation_passed: false,
        });
        assert!(matches!(decision, PolicyDecision::Deny { .. }));
    }

    #[test]
    fn auto_if_low_risk_does_not_auto_run_medium_risk_tickets() {
        let medium = evaluate_run(&PolicyInput {
            source: TicketSource::Human,
            risk_level: RiskLevel::Medium,
            approval_policy: ApprovalPolicy::AutoIfLowRisk,
            has_approval: false,
            has_evidence: false,
            validation_passed: false,
        });
        assert!(matches!(medium, PolicyDecision::RequestApproval { .. }));

        let low = evaluate_run(&PolicyInput {
            source: TicketSource::Human,
            risk_level: RiskLevel::Low,
            approval_policy: ApprovalPolicy::AutoIfLowRisk,
            has_approval: false,
            has_evidence: false,
            validation_passed: false,
        });
        assert_eq!(low, PolicyDecision::Allow);
    }

    #[test]
    fn auto_if_validation_passes_requires_validation_or_approval() {
        let pending = PolicyInput {
            source: TicketSource::Human,
            risk_level: RiskLevel::Low,
            approval_policy: ApprovalPolicy::AutoIfValidationPasses,
            has_approval: false,
            has_evidence: false,
            validation_passed: false,
        };
        assert!(matches!(
            evaluate_run(&pending),
            PolicyDecision::RequestApproval { .. }
        ));
        assert_eq!(
            evaluate_run(&PolicyInput {
                validation_passed: true,
                ..pending
            }),
            PolicyDecision::Allow
        );
    }

    #[test]
    fn human_before_completion_allows_run_but_still_gates_close() {
        let input = PolicyInput {
            source: TicketSource::Human,
            risk_level: RiskLevel::Low,
            approval_policy: ApprovalPolicy::HumanBeforeCompletion,
            has_approval: false,
            has_evidence: true,
            validation_passed: false,
        };
        assert_eq!(evaluate_run(&input), PolicyDecision::Allow);
        assert!(matches!(
            evaluate_close(&input),
            PolicyDecision::RequestApproval { .. }
        ));
    }

    #[test]
    fn provider_policy_recommendations_cannot_weaken_existing_gates() {
        assert!(weakens_approval_policy(
            ApprovalPolicy::HumanBeforeExecute,
            ApprovalPolicy::AlwaysAuto
        ));
        assert!(weakens_approval_policy(
            ApprovalPolicy::ManualOnly,
            ApprovalPolicy::HumanBeforeExecute
        ));
        assert!(weakens_approval_policy(
            ApprovalPolicy::HumanBeforeCompletion,
            ApprovalPolicy::AlwaysAuto
        ));
        assert!(weakens_approval_policy(
            ApprovalPolicy::HumanBeforeCompletion,
            ApprovalPolicy::HumanBeforeExecute
        ));
        assert!(!weakens_approval_policy(
            ApprovalPolicy::AlwaysAuto,
            ApprovalPolicy::HumanBeforeExecute
        ));
        assert!(!weakens_approval_policy(
            ApprovalPolicy::PlanOnly,
            ApprovalPolicy::HumanBeforeWrite
        ));
    }
}
