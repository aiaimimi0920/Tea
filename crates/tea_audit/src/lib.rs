#![forbid(unsafe_code)]

use serde_json::{json, Value};
use tea_core::{Plan, Run, Ticket, TicketAnalysis, TicketComment, TicketEvent};

pub fn render_timeline(ticket: &Ticket, events: &[TicketEvent]) -> String {
    let mut output = format!("# {}\n\n", ticket.title);
    for event in events {
        output.push_str(&format!(
            "- {} {:?} by {:?}\n",
            event.created_at, event.kind, event.actor
        ));
    }
    output
}

pub fn export_json(
    ticket: &Ticket,
    events: &[TicketEvent],
    runs: &[Run],
    comments: &[TicketComment],
    analysis: Option<&TicketAnalysis>,
    plan: Option<&Plan>,
) -> Value {
    json!({
        "ticket": ticket,
        "analysis": analysis,
        "plan": plan,
        "events": events,
        "comments": comments,
        "runs": runs,
    })
}

pub fn render_export_markdown(
    ticket: &Ticket,
    events: &[TicketEvent],
    runs: &[Run],
    comments: &[TicketComment],
    analysis: Option<&TicketAnalysis>,
    plan: Option<&Plan>,
) -> String {
    let mut output = render_timeline(ticket, events);
    if analysis.is_some() || plan.is_some() {
        output.push_str("\n## Decomposition\n\n");
        if let Some(analysis) = analysis {
            output.push_str(&format!(
                "- Intent: {}\n- Workflow: {}\n- Risk: {:?}\n- Confidence: {:.2}\n",
                analysis.intent,
                analysis.recommended_workflow,
                analysis.risk_assessment,
                analysis.confidence
            ));
        }
        if let Some(plan) = plan {
            output.push_str(&format!("\n### Plan\n\n{}\n\n", plan.summary));
            for step in &plan.steps {
                output.push_str(&format!(
                    "- `{}` {}: {}\n",
                    step.id, step.title, step.description
                ));
            }
        }
    }
    if !comments.is_empty() {
        output.push_str("\n## Comments\n\n");
        for comment in comments {
            output.push_str(&format!(
                "- `{}` at {} by {:?}: {}\n",
                comment.id.0, comment.created_at, comment.actor, comment.body
            ));
        }
    }
    if runs.is_empty() {
        return output;
    }

    output.push_str("\n## Runs\n\n");
    for run in runs {
        output.push_str(&format!("- `{}`: {:?}\n", run.id, run.status));
        if let Some(evidence) = &run.evidence {
            output.push_str(&format!("  - Evidence: {}\n", evidence.summary));
            if !evidence.commands.is_empty() {
                output.push_str("  - Commands:\n");
                for command in &evidence.commands {
                    output.push_str(&format!("    - `{command}`\n"));
                }
            }
            if !evidence.artifacts.is_empty() {
                output.push_str("  - Artifacts:\n");
                for artifact in &evidence.artifacts {
                    output.push_str(&format!("    - `{artifact}`\n"));
                }
            }
            if !evidence.risks.is_empty() {
                output.push_str("  - Risks:\n");
                for risk in &evidence.risks {
                    output.push_str(&format!("    - {risk}\n"));
                }
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tea_core::{
        ActorRef, ApprovalPolicy, Plan, PlanStep, RiskLevel, Run, RunEvidence, RunId, RunStatus,
        TicketAnalysis, TicketComment, TicketEventId, TicketEventKind, TicketId, TicketSource,
    };

    #[test]
    fn timeline_contains_event_kind() {
        let ticket = Ticket::new(
            TicketId::new(),
            "Smoke".to_string(),
            "Body".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );
        let event = TicketEvent::new(
            TicketEventId::new(),
            ticket.id.clone(),
            ActorRef::system(),
            TicketEventKind::TicketCreated,
        );

        assert!(render_timeline(&ticket, &[event]).contains("TicketCreated"));
    }

    #[test]
    fn json_export_contains_ticket_events_and_runs() {
        let ticket = Ticket::new(
            TicketId::new(),
            "Smoke".to_string(),
            "Body".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );
        let event = TicketEvent::new(
            TicketEventId::new(),
            ticket.id.clone(),
            ActorRef::system(),
            TicketEventKind::TicketCreated,
        );
        let run = Run {
            id: RunId::new(),
            ticket_id: ticket.id.clone(),
            loom_session_id: Some("loom-session".to_string()),
            status: RunStatus::Succeeded,
            evidence: Some(RunEvidence {
                summary: "validated".to_string(),
                commands: vec!["cargo test".to_string()],
                artifacts: vec!["target/report.json".to_string()],
                risks: vec![],
            }),
        };

        let comment = TicketComment::new(
            ticket.id.clone(),
            ActorRef::human("vmjcv"),
            "Please preserve this review comment in the audit export.".to_string(),
        );

        let analysis = TicketAnalysis {
            intent: "engineering_work_order".to_string(),
            target_components: vec!["Tea".to_string()],
            target_paths: vec!["Tea/crates/tea_api/src/lib.rs".to_string()],
            constraints: vec!["Gateway must not own decomposition".to_string()],
            acceptance_criteria: vec!["Tea stores analysis and plan records".to_string()],
            missing_context: vec![],
            risk_assessment: RiskLevel::Medium,
            confidence: 0.82,
            recommended_policy: ApprovalPolicy::HumanBeforeExecute,
            recommended_workflow: "loom.tea_ticket_decompose.v1".to_string(),
        };
        let plan = Plan {
            summary: "Commit decomposition records into Tea audit export.".to_string(),
            steps: vec![PlanStep {
                id: "export".to_string(),
                title: "Export records".to_string(),
                description: "Include Tea-owned analysis and plan in JSON export.".to_string(),
            }],
            required_tools: vec!["cargo test".to_string()],
            expected_artifacts: vec!["tea-ticket.json".to_string()],
            validation_strategy: vec!["cargo test -p tea_audit".to_string()],
            rollback_strategy: vec!["omit decomposition records from export".to_string()],
            requires_approval_before_execute: true,
        };

        let exported = export_json(
            &ticket,
            &[event],
            &[run],
            &[comment],
            Some(&analysis),
            Some(&plan),
        );
        assert_eq!(exported["ticket"]["title"], "Smoke");
        assert_eq!(exported["events"][0]["kind"], "ticket_created");
        assert_eq!(exported["runs"][0]["evidence"]["summary"], "validated");
        assert_eq!(
            exported["analysis"]["recommended_workflow"],
            "loom.tea_ticket_decompose.v1"
        );
        assert_eq!(exported["plan"]["steps"][0]["id"], "export");
        assert_eq!(
            exported["comments"][0]["body"],
            "Please preserve this review comment in the audit export."
        );
    }

    #[test]
    fn markdown_export_contains_run_evidence_summary() {
        let ticket = Ticket::new(
            TicketId::new(),
            "Smoke".to_string(),
            "Body".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );
        let run = Run {
            id: RunId::new(),
            ticket_id: ticket.id.clone(),
            loom_session_id: Some("loom-session".to_string()),
            status: RunStatus::Succeeded,
            evidence: Some(RunEvidence {
                summary: "mock loom run completed".to_string(),
                commands: vec!["cargo test".to_string()],
                artifacts: vec![],
                risks: vec!["manual review required".to_string()],
            }),
        };

        let comment = TicketComment::new(
            ticket.id.clone(),
            ActorRef::human("vmjcv"),
            "Manual reviewer asked for a rollback note.".to_string(),
        );
        let comment_id = comment.id.0.to_string();

        let analysis = TicketAnalysis {
            intent: "engineering_work_order".to_string(),
            target_components: vec!["Tea".to_string()],
            target_paths: vec![],
            constraints: vec![],
            acceptance_criteria: vec!["Human can audit decomposition".to_string()],
            missing_context: vec![],
            risk_assessment: RiskLevel::Medium,
            confidence: 0.82,
            recommended_policy: ApprovalPolicy::HumanBeforeExecute,
            recommended_workflow: "loom.tea_ticket_decompose.v1".to_string(),
        };
        let plan = Plan {
            summary: "Render decomposition plan in Markdown export.".to_string(),
            steps: vec![PlanStep {
                id: "render".to_string(),
                title: "Render plan".to_string(),
                description: "Show plan summary and steps in audit export.".to_string(),
            }],
            required_tools: vec![],
            expected_artifacts: vec![],
            validation_strategy: vec![],
            rollback_strategy: vec![],
            requires_approval_before_execute: true,
        };

        let markdown = render_export_markdown(
            &ticket,
            &[],
            &[run],
            &[comment],
            Some(&analysis),
            Some(&plan),
        );
        assert!(markdown.contains("## Comments"));
        assert!(markdown.contains("## Decomposition"));
        assert!(markdown.contains("loom.tea_ticket_decompose.v1"));
        assert!(markdown.contains("Render decomposition plan in Markdown export."));
        assert!(markdown.contains(&comment_id));
        assert!(markdown.contains("Manual reviewer asked for a rollback note."));
        assert!(markdown.contains("mock loom run completed"));
        assert!(markdown.contains("cargo test"));
        assert!(markdown.contains("manual review required"));
    }
}
