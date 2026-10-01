use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::post,
    Json, Router,
};
use tea_api::{router, AppState, AuthConfig};
use tea_core::{ActorRef, ApprovalPolicy, Run, RunId, RunStatus, TicketSource, TicketStatus};
use tea_store::{InMemoryTicketStore, SqliteTicketStore, TicketStore};
use tokio::sync::Notify;
use tower::ServiceExt;

// Pause a synthetic Loom response, then change authority before persistence.
// The external request has already happened; only the local commit can be denied.
async fn rejection_during_retry(store: impl TicketStore + Clone + 'static) {
    for by_id in [false, true] {
        for reject in [false, true] {
            let ticket = store
                .create_ticket_with_policy(
                    "Synthetic concurrent retry".into(),
                    "Preserve a newer human decision".into(),
                    TicketSource::Human,
                    ActorRef::system(),
                    ApprovalPolicy::AlwaysAuto,
                )
                .await
                .unwrap();
            store
                .grant_approval(&ticket.id, ActorRef::system())
                .await
                .unwrap();
            let run = Run {
                id: RunId::new(),
                ticket_id: ticket.id.clone(),
                loom_session_id: None,
                status: RunStatus::Failed,
                evidence: None,
            };
            store
                .add_run(&ticket.id, ActorRef::system(), run.clone())
                .await
                .unwrap();
            let entered = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let handler_entered = entered.clone();
            let handler_release = release.clone();
            let remote = Router::new().route(
                "/v1/runs/:id/retry",
                post(move |Json(body): Json<serde_json::Value>| {
                    let entered = handler_entered.clone();
                    let release = handler_release.clone();
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        let mut run: Run = serde_json::from_value(body["run"].clone()).unwrap();
                        run.status = RunStatus::Retrying;
                        Json(run)
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, remote).await.unwrap() });
            let app = router(AppState::new(
                store.clone(),
                tea_brain::TemplateBrainProvider,
                tea_loom::HttpLoomClient::new(url, None),
                AuthConfig::new("test-token".into()),
            ));
            let uri = if by_id {
                format!("/v1/runs/{}/retry", run.id)
            } else {
                format!("/v1/tickets/{}/retry", ticket.id)
            };
            let response = tokio::spawn(
                app.oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("authorization", "Bearer test-token")
                        .body(Body::empty())
                        .unwrap(),
                ),
            );
            tokio::time::timeout(Duration::from_secs(5), entered.notified())
                .await
                .unwrap();
            if reject {
                store
                    .reject_approval(&ticket.id, ActorRef::system(), "Changed decision".into())
                    .await
                    .unwrap();
            } else {
                store
                    .set_approval_policy(
                        &ticket.id,
                        ActorRef::system(),
                        ApprovalPolicy::HumanBeforeExecute,
                    )
                    .await
                    .unwrap();
            }
            assert!(!store.has_approval(&ticket.id).await.unwrap());
            let before_ticket = store.get_ticket(&ticket.id).await.unwrap();
            let before_events = store.ticket_events(&ticket.id).await.unwrap();
            release.notify_one();
            let response = tokio::time::timeout(Duration::from_secs(5), response)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            assert_eq!(
                response.status(),
                if reject {
                    StatusCode::CONFLICT
                } else {
                    StatusCode::FORBIDDEN
                }
            );
            assert_eq!(store.get_run(&run.id).await.unwrap(), run);
            assert_eq!(store.get_ticket(&ticket.id).await.unwrap(), before_ticket);
            assert_eq!(
                store.ticket_events(&ticket.id).await.unwrap(),
                before_events
            );
            if reject {
                assert_eq!(before_ticket.status, TicketStatus::Blocked);
            }
            // Non-CAS updates share the same atomic guard rather than bypassing it.
            let mut retrying = run.clone();
            retrying.status = RunStatus::Retrying;
            assert!(store
                .update_run(&ticket.id, ActorRef::system(), retrying)
                .await
                .is_err());
            assert_eq!(store.get_run(&run.id).await.unwrap(), run);
            server.abort();
            let _ = server.await;
        }
    }
}

#[tokio::test]
async fn memory_preserves_newer_retry_authorization() {
    rejection_during_retry(InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_preserves_newer_retry_authorization() {
    rejection_during_retry(SqliteTicketStore::open(":memory:").unwrap()).await;
}
