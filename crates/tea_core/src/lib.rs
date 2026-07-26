#![forbid(unsafe_code)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ParseIdError {
    #[error("invalid uuid: {0}")]
    InvalidUuid(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TicketId(pub Uuid);

impl TicketId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TicketId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TicketId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for TicketId {
    type Err = ParseIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| ParseIdError::InvalidUuid(value.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TicketEventId(pub Uuid);

impl TicketEventId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TicketEventId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CommentId(pub Uuid);

impl CommentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CommentId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RunId(pub Uuid);

impl RunId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for RunId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for RunId {
    type Err = ParseIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|_| ParseIdError::InvalidUuid(value.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ActorRef {
    System,
    Human(String),
    Hook(String),
    Agent(String),
    Loom(String),
    Policy(String),
}

impl ActorRef {
    pub fn system() -> Self {
        Self::System
    }

    pub fn human(id: impl Into<String>) -> Self {
        Self::Human(id.into())
    }

    pub fn hook(id: impl Into<String>) -> Self {
        Self::Hook(id.into())
    }

    pub fn agent(id: impl Into<String>) -> Self {
        Self::Agent(id.into())
    }

    pub fn loom(id: impl Into<String>) -> Self {
        Self::Loom(id.into())
    }

    pub fn policy(id: impl Into<String>) -> Self {
        Self::Policy(id.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketSource {
    Human,
    Hook,
    Api,
    System,
}

impl TicketSource {
    pub fn as_label(self) -> &'static str {
        match self {
            TicketSource::Human => "human",
            TicketSource::Hook => "hook",
            TicketSource::Api => "api",
            TicketSource::System => "system",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    Draft,
    Open,
    NeedsInfo,
    Analyzing,
    AnalysisReady,
    Planning,
    PlanReady,
    AwaitingApproval,
    Approved,
    Running,
    Blocked,
    Failed,
    NeedsReview,
    Completed,
    Accepted,
    Closed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    PlanOnly,
    HumanBeforeExecute,
    HumanBeforeWrite,
    HumanBeforeExternalNetwork,
    HumanBeforeDestructiveAction,
    HumanBeforeCompletion,
    AutoIfLowRisk,
    AutoIfValidationPasses,
    ManualOnly,
    AlwaysAuto,
}

impl ApprovalPolicy {
    pub fn as_label_value(self) -> &'static str {
        match self {
            ApprovalPolicy::PlanOnly => "plan-only",
            ApprovalPolicy::HumanBeforeExecute => "human-before-execute",
            ApprovalPolicy::HumanBeforeWrite => "human-before-write",
            ApprovalPolicy::HumanBeforeExternalNetwork => "human-before-external-network",
            ApprovalPolicy::HumanBeforeDestructiveAction => "human-before-destructive-action",
            ApprovalPolicy::HumanBeforeCompletion => "human-before-completion",
            ApprovalPolicy::AutoIfLowRisk => "auto-if-low-risk",
            ApprovalPolicy::AutoIfValidationPasses => "auto-if-validation-passes",
            ApprovalPolicy::ManualOnly => "manual-only",
            ApprovalPolicy::AlwaysAuto => "always-auto",
        }
    }

    pub fn as_label(self) -> String {
        format!("policy:{}", self.as_label_value())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Stopped,
    Retrying,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ticket {
    pub id: TicketId,
    pub title: String,
    pub description: String,
    pub source: TicketSource,
    pub status: TicketStatus,
    pub priority: String,
    pub labels: Vec<String>,
    pub owner_human_id: Option<String>,
    pub delegated_agent_id: Option<String>,
    pub approval_policy: ApprovalPolicy,
    pub risk_level: RiskLevel,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Optional caller-provided overrides applied when a ticket is created.
///
/// `None`/empty fields keep the historical defaults (priority `"normal"`,
/// only source/policy-derived labels), so existing call sites that pass
/// `TicketCreateOptions::default()` behave exactly as before.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketCreateOptions {
    pub priority: Option<String>,
    pub labels: Vec<String>,
}

impl Ticket {
    pub fn default_approval_policy_for_source(source: TicketSource) -> ApprovalPolicy {
        match source {
            TicketSource::Hook => ApprovalPolicy::PlanOnly,
            _ => ApprovalPolicy::HumanBeforeExecute,
        }
    }

    pub fn new(
        id: TicketId,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
    ) -> Self {
        Self::new_with_approval_policy(
            id,
            title,
            description,
            source,
            actor,
            Self::default_approval_policy_for_source(source),
        )
    }

    pub fn new_with_approval_policy(
        id: TicketId,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
        approval_policy: ApprovalPolicy,
    ) -> Self {
        Self::new_with_options(
            id,
            title,
            description,
            source,
            actor,
            approval_policy,
            TicketCreateOptions::default(),
        )
    }

    pub fn new_with_options(
        id: TicketId,
        title: String,
        description: String,
        source: TicketSource,
        actor: ActorRef,
        approval_policy: ApprovalPolicy,
        options: TicketCreateOptions,
    ) -> Self {
        let now = Utc::now();
        let owner_human_id = match actor {
            ActorRef::Human(id) => Some(id),
            _ => None,
        };
        let mut labels = vec![format!("source:{}", source.as_label())];
        if source == TicketSource::Hook {
            labels.push("context:untrusted".to_string());
        }
        for label in options.labels {
            let trimmed = label.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !labels.iter().any(|existing| existing == trimmed) {
                labels.push(trimmed.to_string());
            }
        }
        sync_policy_label(&mut labels, approval_policy);

        let priority = options
            .priority
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "normal".to_string());

        Self {
            id,
            title,
            description,
            source,
            status: TicketStatus::Open,
            priority,
            labels,
            owner_human_id,
            delegated_agent_id: None,
            approval_policy,
            risk_level: RiskLevel::Medium,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn set_approval_policy(&mut self, approval_policy: ApprovalPolicy) {
        self.approval_policy = approval_policy;
        sync_policy_label(&mut self.labels, approval_policy);
        self.touch();
    }

    /// Apply operator edits to mutable ticket fields.
    ///
    /// Only fields present in `edits` are changed. Editing `labels` replaces
    /// the operator-facing labels but preserves system-derived labels
    /// (`source:`, `policy:`, `context:`), so an edit cannot drop the
    /// provenance/policy provenance that Tea relies on. Returns `true` when
    /// any field actually changed.
    pub fn apply_edits(&mut self, edits: TicketEdits) -> bool {
        let mut changed = false;

        if let Some(title) = edits.title {
            let trimmed = title.trim();
            if !trimmed.is_empty() && trimmed != self.title {
                self.title = trimmed.to_string();
                changed = true;
            }
        }

        if let Some(description) = edits.description {
            if description != self.description {
                self.description = description;
                changed = true;
            }
        }

        if let Some(priority) = edits.priority {
            let trimmed = priority.trim();
            if !trimmed.is_empty() && trimmed != self.priority {
                self.priority = trimmed.to_string();
                changed = true;
            }
        }

        if let Some(new_labels) = edits.labels {
            let mut merged: Vec<String> = self
                .labels
                .iter()
                .filter(|label| is_system_label(label))
                .cloned()
                .collect();
            for label in new_labels {
                let trimmed = label.trim();
                if trimmed.is_empty() || is_system_label(trimmed) {
                    continue;
                }
                if !merged.iter().any(|existing| existing == trimmed) {
                    merged.push(trimmed.to_string());
                }
            }
            if merged != self.labels {
                self.labels = merged;
                changed = true;
            }
        }

        if changed {
            self.touch();
        }
        changed
    }

    pub fn touch(&mut self) {
        self.updated_at = Utc::now();
    }
}

/// Optional operator edits to mutable ticket fields. `None` fields are left
/// unchanged; a `Some(labels)` replaces operator labels while preserving
/// system-derived labels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TicketEdits {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub labels: Option<Vec<String>>,
}

impl TicketEdits {
    /// Returns `true` when no field would change anything.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.description.is_none()
            && self.priority.is_none()
            && self.labels.is_none()
    }
}

/// System-derived labels that operators cannot set or remove through edits.
fn is_system_label(label: &str) -> bool {
    label.starts_with("source:") || label.starts_with("policy:") || label.starts_with("context:")
}

fn sync_policy_label(labels: &mut Vec<String>, approval_policy: ApprovalPolicy) {
    labels.retain(|label| !label.starts_with("policy:"));
    labels.push(approval_policy.as_label());
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketComment {
    pub id: CommentId,
    pub ticket_id: TicketId,
    pub actor: ActorRef,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

impl TicketComment {
    pub fn new(ticket_id: TicketId, actor: ActorRef, body: String) -> Self {
        Self {
            id: CommentId::new(),
            ticket_id,
            actor,
            body,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketEventKind {
    TicketCreated,
    CommentAdded,
    TicketAnalyzed,
    PlanProposed,
    PolicyUpdated,
    TicketEdited,
    ApprovalRequested,
    ApprovalGranted,
    ApprovalRejected,
    RunQueued,
    RunStarted,
    RunEventReceived,
    RunFailed,
    RunSucceeded,
    EvidenceAttached,
    ReviewRequested,
    HumanAccepted,
    TicketClosed,
    TicketCancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketEvent {
    pub id: TicketEventId,
    pub ticket_id: TicketId,
    pub actor: ActorRef,
    pub kind: TicketEventKind,
    pub created_at: DateTime<Utc>,
}

impl TicketEvent {
    pub fn new(
        id: TicketEventId,
        ticket_id: TicketId,
        actor: ActorRef,
        kind: TicketEventKind,
    ) -> Self {
        Self {
            id,
            ticket_id,
            actor,
            kind,
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TicketAnalysis {
    pub intent: String,
    pub target_components: Vec<String>,
    pub target_paths: Vec<String>,
    pub constraints: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub missing_context: Vec<String>,
    pub risk_assessment: RiskLevel,
    pub confidence: f32,
    pub recommended_policy: ApprovalPolicy,
    pub recommended_workflow: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub summary: String,
    pub steps: Vec<PlanStep>,
    pub required_tools: Vec<String>,
    pub expected_artifacts: Vec<String>,
    pub validation_strategy: Vec<String>,
    pub rollback_strategy: Vec<String>,
    pub requires_approval_before_execute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub ticket_id: TicketId,
    pub loom_session_id: Option<String>,
    pub status: RunStatus,
    pub evidence: Option<RunEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunEvidence {
    pub summary: String,
    pub commands: Vec<String>,
    pub artifacts: Vec<String>,
    pub risks: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticket_creation_defaults_to_open() {
        let actor = ActorRef::human("vmjcv");
        let ticket = Ticket::new(
            TicketId::new(),
            "Smoke".to_string(),
            "Analyze the repo and produce a plan.".to_string(),
            TicketSource::Human,
            actor,
        );

        assert_eq!(ticket.status, TicketStatus::Open);
        assert_eq!(ticket.source, TicketSource::Human);
        assert_eq!(ticket.owner_human_id.as_deref(), Some("vmjcv"));
        assert_eq!(ticket.approval_policy, ApprovalPolicy::HumanBeforeExecute);
    }

    #[test]
    fn hook_ticket_defaults_to_plan_only() {
        let ticket = Ticket::new(
            TicketId::new(),
            "Hook request".to_string(),
            "Captured request".to_string(),
            TicketSource::Hook,
            ActorRef::hook("desktop"),
        );

        assert_eq!(ticket.approval_policy, ApprovalPolicy::PlanOnly);
        assert!(ticket.labels.contains(&"source:hook".to_string()));
        assert!(ticket.labels.contains(&"policy:plan-only".to_string()));
        assert!(ticket.labels.contains(&"context:untrusted".to_string()));
    }

    #[test]
    fn event_requires_actor() {
        let event = TicketEvent::new(
            TicketEventId::new(),
            TicketId::new(),
            ActorRef::system(),
            TicketEventKind::TicketCreated,
        );

        assert_eq!(event.actor, ActorRef::system());
    }

    #[test]
    fn apply_edits_updates_only_provided_fields() {
        let mut ticket = Ticket::new(
            TicketId::new(),
            "Original".to_string(),
            "Original body".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );

        let changed = ticket.apply_edits(TicketEdits {
            title: Some("Updated title".to_string()),
            description: None,
            priority: Some("high".to_string()),
            labels: None,
        });

        assert!(changed);
        assert_eq!(ticket.title, "Updated title");
        assert_eq!(ticket.description, "Original body");
        assert_eq!(ticket.priority, "high");
    }

    #[test]
    fn apply_edits_preserves_system_labels() {
        let mut ticket = Ticket::new(
            TicketId::new(),
            "Hook request".to_string(),
            "Captured request".to_string(),
            TicketSource::Hook,
            ActorRef::hook("desktop"),
        );

        let changed = ticket.apply_edits(TicketEdits {
            title: None,
            description: None,
            priority: None,
            labels: Some(vec!["area:auth".to_string(), "needs-triage".to_string()]),
        });

        assert!(changed);
        // System-derived labels survive an operator label edit.
        assert!(ticket.labels.contains(&"source:hook".to_string()));
        assert!(ticket.labels.contains(&"policy:plan-only".to_string()));
        assert!(ticket.labels.contains(&"context:untrusted".to_string()));
        // Operator labels are applied.
        assert!(ticket.labels.contains(&"area:auth".to_string()));
        assert!(ticket.labels.contains(&"needs-triage".to_string()));
    }

    #[test]
    fn apply_edits_reports_no_change_when_empty() {
        let mut ticket = Ticket::new(
            TicketId::new(),
            "Original".to_string(),
            "Original body".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );

        let changed = ticket.apply_edits(TicketEdits::default());
        assert!(!changed);
    }
}
