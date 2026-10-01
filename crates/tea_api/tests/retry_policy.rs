use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    routing::post,
    Json, Router,
};
use tea_api::{router, AppState, AuthConfig};
use tea_core::{
    ActorRef, ApprovalPolicy, RiskLevel, Run, RunId, RunStatus, TicketAnalysis, TicketSource,
};
use tea_store::{InMemoryTicketStore, SqliteTicketStore, TicketStore};
use tower::ServiceExt;

#[derive(Clone, Copy, Debug)]
enum Gate {
    Policy(ApprovalPolicy, bool, RiskLevel),
    Rejected,
    NeedsInfo,
}

async fn prepare(store: &impl TicketStore, status: RunStatus, gate: Gate) -> Run {
    let ticket = store
        .create_ticket_with_policy(
            "Synthetic retry permission".into(),
            "Retry must respect current authorization".into(),
            TicketSource::Human,
            ActorRef::system(),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    let run = Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: None,
        status,
        evidence: None,
    };
    store
        .add_run(&ticket.id, ActorRef::system(), run.clone())
        .await
        .unwrap();
    let (policy, approved, risk) = match gate {
        Gate::Policy(policy, approved, risk) => (policy, approved, risk),
        _ => (ApprovalPolicy::AlwaysAuto, false, RiskLevel::Low),
    };
    if policy != ApprovalPolicy::AlwaysAuto {
        store
            .grant_approval(&ticket.id, ActorRef::system())
            .await
            .unwrap();
    }
    // Analysis can tighten both risk and policy, invalidating earlier approvals.
    store
        .set_analysis(
            &ticket.id,
            ActorRef::system(),
            TicketAnalysis {
                intent: "Synthetic gate".into(),
                target_components: vec![],
                target_paths: vec![],
                constraints: vec![],
                acceptance_criteria: vec![],
                missing_context: if matches!(gate, Gate::NeedsInfo) {
                    vec!["Missing repository".into()]
                } else {
                    vec![]
                },
                risk_assessment: risk,
                confidence: 1.0,
                recommended_policy: policy,
                recommended_workflow: "test".into(),
            },
        )
        .await
        .unwrap();
    if policy != ApprovalPolicy::AlwaysAuto {
        assert!(!store.has_approval(&ticket.id).await.unwrap());
    }
    if approved {
        store
            .grant_approval(&ticket.id, ActorRef::system())
            .await
            .unwrap();
    }
    if matches!(gate, Gate::Rejected) {
        store
            .reject_approval(&ticket.id, ActorRef::system(), "Do not execute".into())
            .await
            .unwrap();
    }
    run
}

async fn upstream(
    State(calls): State<Arc<AtomicUsize>>,
    Json(body): Json<serde_json::Value>,
) -> Json<Run> {
    calls.fetch_add(1, Ordering::SeqCst);
    let mut run: Run = serde_json::from_value(body["run"].clone()).unwrap();
    run.status = if run.status.can_retry() {
        RunStatus::Retrying
    } else {
        RunStatus::Stopped
    };
    Json(run)
}

async fn request(app: Router, run: &Run, by_id: bool, action: &str) -> StatusCode {
    let uri = if by_id {
        format!("/v1/runs/{}/{action}", run.id)
    } else {
        format!("/v1/tickets/{}/{action}", run.ticket_id)
    };
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", "Bearer test-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
    .status()
}

async fn policy_matrix(store: impl TicketStore + Clone + 'static) {
    use ApprovalPolicy::*;
    use RiskLevel::*;
    let cases = [
        (Gate::Policy(ManualOnly, false, Low), StatusCode::FORBIDDEN),
        (Gate::Policy(ManualOnly, true, Low), StatusCode::FORBIDDEN),
        (
            Gate::Policy(HumanBeforeExecute, false, Low),
            StatusCode::FORBIDDEN,
        ),
        (Gate::Policy(HumanBeforeExecute, true, Low), StatusCode::OK),
        (Gate::Policy(AlwaysAuto, false, Low), StatusCode::OK),
        (Gate::Policy(AlwaysAuto, false, High), StatusCode::FORBIDDEN),
        (Gate::Policy(AlwaysAuto, true, High), StatusCode::OK),
        (
            Gate::Policy(AutoIfLowRisk, false, Medium),
            StatusCode::FORBIDDEN,
        ),
        (Gate::Policy(AutoIfLowRisk, false, Low), StatusCode::OK),
        (
            Gate::Policy(AutoIfValidationPasses, false, Low),
            StatusCode::FORBIDDEN,
        ),
        (
            Gate::Policy(AutoIfValidationPasses, true, Low),
            StatusCode::OK,
        ),
        (
            Gate::Policy(HumanBeforeCompletion, false, Low),
            StatusCode::OK,
        ),
        (Gate::Rejected, StatusCode::CONFLICT),
        (Gate::NeedsInfo, StatusCode::CONFLICT),
    ];
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let remote = Router::new()
        .route("/v1/runs/:id/retry", post(upstream))
        .route("/v1/runs/:id/stop", post(upstream))
        .with_state(calls.clone());
    let server = tokio::spawn(async move { axum::serve(listener, remote).await.unwrap() });
    let app = router(AppState::new(
        store.clone(),
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(url, None),
        AuthConfig::new("test-token".into()),
    ));
    for by_id in [false, true] {
        for status in [RunStatus::Failed, RunStatus::Stopped] {
            for (gate, expected) in cases {
                let run = prepare(&store, status, gate).await;
                let before_ticket = store.get_ticket(&run.ticket_id).await.unwrap();
                let before_events = store.ticket_events(&run.ticket_id).await.unwrap();
                let before_calls = calls.load(Ordering::SeqCst);
                assert_eq!(
                    request(app.clone(), &run, by_id, "retry").await,
                    expected,
                    "gate {gate:?}, route by id {by_id}, run {status:?}"
                );
                assert_eq!(
                    calls.load(Ordering::SeqCst) - before_calls,
                    usize::from(expected == StatusCode::OK)
                );
                if expected != StatusCode::OK {
                    assert_eq!(store.get_run(&run.id).await.unwrap(), run);
                    assert_eq!(
                        store.get_ticket(&run.ticket_id).await.unwrap(),
                        before_ticket
                    );
                    assert_eq!(
                        store.ticket_events(&run.ticket_id).await.unwrap(),
                        before_events
                    );
                } else {
                    assert_eq!(
                        store.get_run(&run.id).await.unwrap().status,
                        RunStatus::Retrying
                    );
                }
            }
        }
        // Stop reduces execution, so a later restriction must not block it.
        for gate in [
            Gate::Policy(ManualOnly, false, Low),
            Gate::Rejected,
            Gate::NeedsInfo,
        ] {
            let run = prepare(&store, RunStatus::Running, gate).await;
            assert_eq!(
                request(app.clone(), &run, by_id, "stop").await,
                StatusCode::OK
            );
            assert_eq!(
                store.get_run(&run.id).await.unwrap().status,
                RunStatus::Stopped
            );
            let before_calls = calls.load(Ordering::SeqCst);
            let expected = if matches!(gate, Gate::Rejected | Gate::NeedsInfo) {
                StatusCode::CONFLICT
            } else {
                StatusCode::FORBIDDEN
            };
            assert_eq!(request(app.clone(), &run, by_id, "retry").await, expected);
            assert_eq!(calls.load(Ordering::SeqCst), before_calls);
        }
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn memory_retry_rechecks_current_policy() {
    policy_matrix(InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_retry_rechecks_current_policy() {
    policy_matrix(SqliteTicketStore::open(":memory:").unwrap()).await;
}
