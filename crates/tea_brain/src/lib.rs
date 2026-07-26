#![forbid(unsafe_code)]

use async_trait::async_trait;
use tea_core::{
    ApprovalPolicy, Plan, PlanStep, RiskLevel, Ticket, TicketAnalysis, TicketComment, TicketSource,
};
use thiserror::Error;

pub const TEA_TICKET_DECOMPOSE_CAPABILITY: &str = "tea.ticket.decompose.v1";
pub const LOOM_TEA_TICKET_DECOMPOSE_WORKFLOW: &str = "loom.tea_ticket_decompose.v1";

#[derive(Debug, Error)]
pub enum BrainError {
    #[error("BrainProvider request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("BrainProvider returned invalid response: {0}")]
    InvalidResponse(String),
    #[error("BrainProvider unavailable: {0}")]
    ProviderUnavailable(String),
}

pub type BrainResult<T> = Result<T, BrainError>;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BrainProviderMetadata {
    pub mode: String,
    pub capability: String,
}

impl BrainProviderMetadata {
    pub fn new(mode: impl Into<String>, capability: impl Into<String>) -> Self {
        Self {
            mode: mode.into(),
            capability: capability.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecomposeTicketRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub ticket: Ticket,
    pub comments: Vec<TicketComment>,
    pub policy: DecomposePolicy,
    pub context: DecomposeContext,
}

impl DecomposeTicketRequest {
    pub fn new(ticket: Ticket, comments: Vec<TicketComment>, context: DecomposeContext) -> Self {
        Self {
            schema_version: 1,
            request_id: uuid::Uuid::new_v4().to_string(),
            policy: DecomposePolicy {
                approval_policy: ticket.approval_policy,
                terminal_state_guard: true,
            },
            ticket,
            comments,
            context,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecomposePolicy {
    pub approval_policy: ApprovalPolicy,
    pub terminal_state_guard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DecomposeContext {
    pub workspace_root: Option<String>,
    pub platform_mode: String,
    pub requested_by: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DecomposeTicketProposal {
    pub schema_version: u32,
    pub proposal_id: String,
    pub analysis: TicketAnalysis,
    pub plan: Plan,
    pub requires_human_review: bool,
    pub notes: Vec<String>,
}

#[async_trait]
pub trait TeaBrainProvider: Clone + Send + Sync + 'static {
    fn metadata(&self) -> BrainProviderMetadata;

    async fn decompose_ticket(
        &self,
        request: DecomposeTicketRequest,
    ) -> BrainResult<DecomposeTicketProposal>;
}

#[derive(Debug, Clone, Default)]
pub struct TemplateBrainProvider;

#[async_trait]
impl TeaBrainProvider for TemplateBrainProvider {
    fn metadata(&self) -> BrainProviderMetadata {
        BrainProviderMetadata::new("template", TEA_TICKET_DECOMPOSE_CAPABILITY)
    }

    async fn decompose_ticket(
        &self,
        request: DecomposeTicketRequest,
    ) -> BrainResult<DecomposeTicketProposal> {
        Ok(template_proposal(&request))
    }
}

#[derive(Debug, Clone, Default)]
pub struct ManualBrainProvider;

#[async_trait]
impl TeaBrainProvider for ManualBrainProvider {
    fn metadata(&self) -> BrainProviderMetadata {
        BrainProviderMetadata::new("manual", TEA_TICKET_DECOMPOSE_CAPABILITY)
    }

    async fn decompose_ticket(
        &self,
        _request: DecomposeTicketRequest,
    ) -> BrainResult<DecomposeTicketProposal> {
        Err(BrainError::ProviderUnavailable(
            "manual BrainProvider requires a human-authored Tea analysis and plan".to_string(),
        ))
    }
}

#[derive(Debug, Clone)]
pub struct LoomCapabilityBrainProvider {
    base_url: String,
    auth_token: Option<String>,
    http: reqwest::Client,
}

impl LoomCapabilityBrainProvider {
    pub fn new(base_url: String, auth_token: Option<String>) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth_token,
            http: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth_token {
            Some(token) if !token.trim().is_empty() => request.bearer_auth(token),
            _ => request,
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct LoomInvokeRequest<'a> {
    #[serde(rename = "requestId")]
    request_id: &'a str,
    caller: &'a str,
    capability: &'a str,
    input: &'a DecomposeTicketRequest,
}

#[derive(Debug, serde::Deserialize)]
struct LoomInvokeResponse {
    status: String,
    output: LoomInvokeOutput,
}

#[derive(Debug, serde::Deserialize)]
struct LoomInvokeOutput {
    proposal: DecomposeTicketProposal,
}

#[async_trait]
impl TeaBrainProvider for LoomCapabilityBrainProvider {
    fn metadata(&self) -> BrainProviderMetadata {
        BrainProviderMetadata::new("loom", TEA_TICKET_DECOMPOSE_CAPABILITY)
    }

    async fn decompose_ticket(
        &self,
        request: DecomposeTicketRequest,
    ) -> BrainResult<DecomposeTicketProposal> {
        let response = self
            .authorize(
                self.http
                    .post(self.url("/v1/invoke"))
                    .json(&LoomInvokeRequest {
                        request_id: &request.request_id,
                        caller: "tea",
                        capability: TEA_TICKET_DECOMPOSE_CAPABILITY,
                        input: &request,
                    }),
            )
            .send()
            .await?
            .error_for_status()?
            .json::<LoomInvokeResponse>()
            .await?;
        if response.status != "succeeded" {
            return Err(BrainError::InvalidResponse(format!(
                "Loom invoke status was {}",
                response.status
            )));
        }
        Ok(response.output.proposal)
    }
}

#[derive(Debug, Clone)]
pub enum RuntimeTeaBrainProvider {
    Template(TemplateBrainProvider),
    Manual(ManualBrainProvider),
    Loom(LoomCapabilityBrainProvider),
}

impl RuntimeTeaBrainProvider {
    pub fn template() -> Self {
        Self::Template(TemplateBrainProvider)
    }

    pub fn manual() -> Self {
        Self::Manual(ManualBrainProvider)
    }

    pub fn loom(base_url: String, auth_token: Option<String>) -> Self {
        Self::Loom(LoomCapabilityBrainProvider::new(base_url, auth_token))
    }
}

#[async_trait]
impl TeaBrainProvider for RuntimeTeaBrainProvider {
    fn metadata(&self) -> BrainProviderMetadata {
        match self {
            Self::Template(provider) => provider.metadata(),
            Self::Manual(provider) => provider.metadata(),
            Self::Loom(provider) => provider.metadata(),
        }
    }

    async fn decompose_ticket(
        &self,
        request: DecomposeTicketRequest,
    ) -> BrainResult<DecomposeTicketProposal> {
        match self {
            Self::Template(provider) => provider.decompose_ticket(request).await,
            Self::Manual(provider) => provider.decompose_ticket(request).await,
            Self::Loom(provider) => provider.decompose_ticket(request).await,
        }
    }
}

fn template_proposal(request: &DecomposeTicketRequest) -> DecomposeTicketProposal {
    let ticket = &request.ticket;
    let non_whitespace = ticket
        .description
        .chars()
        .filter(|character| !character.is_whitespace())
        .count();
    let missing_context = if non_whitespace < 12 {
        vec!["description is too short".to_string()]
    } else {
        Vec::new()
    };
    let recommended_policy = match ticket.source {
        TicketSource::Hook => ApprovalPolicy::PlanOnly,
        _ => ApprovalPolicy::HumanBeforeExecute,
    };
    let analysis = TicketAnalysis {
        intent: "engineering_work_order".to_string(),
        target_components: vec!["Tea".to_string()],
        target_paths: vec![],
        constraints: vec![
            "Loom must not mutate Tea ticket state directly".to_string(),
            "Tea validates, stores, and governs decomposition records".to_string(),
        ],
        acceptance_criteria: vec![
            "analysis and plan are returned as one proposal".to_string(),
            "Tea commits accepted records into its own timeline".to_string(),
            "evidence is attached before human acceptance".to_string(),
        ],
        missing_context,
        risk_assessment: RiskLevel::Medium,
        confidence: 0.8,
        recommended_policy,
        recommended_workflow: LOOM_TEA_TICKET_DECOMPOSE_WORKFLOW.to_string(),
    };
    let plan = Plan {
        summary: format!("Decompose Tea work order: {}", ticket.title),
        steps: vec![
            PlanStep {
                id: "inspect-context".to_string(),
                title: "Inspect context".to_string(),
                description: "Read the ticket, comments, policy, and available context."
                    .to_string(),
            },
            PlanStep {
                id: "propose-plan".to_string(),
                title: "Propose plan".to_string(),
                description: "Create a bounded implementation approach for Tea to store."
                    .to_string(),
            },
            PlanStep {
                id: "validate".to_string(),
                title: "Validate".to_string(),
                description: "Run relevant checks and attach evidence.".to_string(),
            },
        ],
        required_tools: vec!["loom.run".to_string()],
        expected_artifacts: vec![
            "Tea analysis record".to_string(),
            "Tea plan record".to_string(),
        ],
        validation_strategy: analysis.acceptance_criteria.clone(),
        rollback_strategy: vec!["leave ticket blocked with proposal evidence".to_string()],
        requires_approval_before_execute: !matches!(
            analysis.recommended_policy,
            ApprovalPolicy::AlwaysAuto | ApprovalPolicy::AutoIfLowRisk
        ),
    };
    DecomposeTicketProposal {
        schema_version: 1,
        proposal_id: uuid::Uuid::new_v4().to_string(),
        analysis,
        plan,
        requires_human_review: true,
        notes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, routing::post, Json, Router};
    use std::sync::{Arc, Mutex};
    use tea_core::{ActorRef, TicketId};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn template_provider_returns_one_analysis_plan_proposal() {
        let ticket = Ticket::new(
            TicketId::new(),
            "Template".to_string(),
            "Create a safe plan through the local template provider.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );
        let request = DecomposeTicketRequest::new(
            ticket,
            Vec::new(),
            DecomposeContext {
                workspace_root: None,
                platform_mode: "test".to_string(),
                requested_by: "test".to_string(),
            },
        );

        let proposal = TemplateBrainProvider
            .decompose_ticket(request)
            .await
            .unwrap();

        assert_eq!(proposal.analysis.intent, "engineering_work_order");
        assert_eq!(
            proposal.analysis.recommended_workflow,
            LOOM_TEA_TICKET_DECOMPOSE_WORKFLOW
        );
        assert_eq!(proposal.plan.steps.len(), 3);
        assert!(proposal.plan.requires_approval_before_execute);
    }

    #[tokio::test]
    async fn loom_provider_invokes_tea_decompose_capability() {
        #[derive(Clone, Default)]
        struct Capture {
            authorization: Arc<Mutex<Option<String>>>,
            body: Arc<Mutex<Option<serde_json::Value>>>,
        }

        async fn invoke_handler(
            State(capture): State<Capture>,
            headers: axum::http::HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Json<serde_json::Value> {
            *capture.authorization.lock().unwrap() = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            *capture.body.lock().unwrap() = Some(body);
            Json(serde_json::json!({
                "requestId": "request-1",
                "status": "succeeded",
                "output": {
                    "proposal": {
                        "schema_version": 1,
                        "proposal_id": "proposal-1",
                        "analysis": {
                            "intent": "engineering_work_order",
                            "target_components": ["Tea"],
                            "target_paths": [],
                            "constraints": ["test"],
                            "acceptance_criteria": ["pass"],
                            "missing_context": [],
                            "risk_assessment": "medium",
                            "confidence": 0.82,
                            "recommended_policy": "human_before_execute",
                            "recommended_workflow": "loom.tea_ticket_decompose.v1"
                        },
                        "plan": {
                            "summary": "remote proposal",
                            "steps": [
                                {
                                    "id": "inspect-context",
                                    "title": "Inspect context",
                                    "description": "Read context."
                                }
                            ],
                            "required_tools": ["loom.run"],
                            "expected_artifacts": ["evidence"],
                            "validation_strategy": ["pass"],
                            "rollback_strategy": ["stop"],
                            "requires_approval_before_execute": true
                        },
                        "requires_human_review": true,
                        "notes": []
                    }
                }
            }))
        }

        let capture = Capture::default();
        let app = Router::new()
            .route("/v1/invoke", post(invoke_handler))
            .with_state(capture.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let ticket = Ticket::new(
            TicketId::new(),
            "Remote".to_string(),
            "Ask Loom to decompose this ticket.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );
        let request = DecomposeTicketRequest::new(
            ticket.clone(),
            Vec::new(),
            DecomposeContext {
                workspace_root: None,
                platform_mode: "test".to_string(),
                requested_by: "test".to_string(),
            },
        );
        let client = LoomCapabilityBrainProvider::new(
            format!("http://{address}"),
            Some("loom-token".to_string()),
        );

        let proposal = client.decompose_ticket(request).await.unwrap();

        assert_eq!(proposal.plan.summary, "remote proposal");
        assert_eq!(
            capture.authorization.lock().unwrap().as_deref(),
            Some("Bearer loom-token")
        );
        let body = capture.body.lock().unwrap().clone().unwrap();
        assert_eq!(body["caller"], "tea");
        assert_eq!(body["capability"], TEA_TICKET_DECOMPOSE_CAPABILITY);
        assert_eq!(body["input"]["ticket"]["id"], serde_json::json!(ticket.id));
    }
}
