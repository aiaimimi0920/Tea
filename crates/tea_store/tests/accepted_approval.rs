use tea_core::{
    ActorRef, ApprovalPolicy, Run, RunEvidence, RunId, RunStatus, TicketId, TicketSource,
    TicketStatus,
};
use tea_store::{InMemoryTicketStore, SqliteTicketStore, StoreError, TicketStore};

// Exercise the existing acceptance semantics without weakening closure's gate.
// Completion approval must record permission without regressing the review stage.
async fn prepare_completion_reviews(store: &impl TicketStore) -> Vec<(TicketId, TicketStatus)> {
    let mut reviews = Vec::new();
    for accept_first in [false, true] {
        let ticket = store
            .create_ticket_with_policy(
                "Synthetic completion review".into(),
                "Preserve the review milestone when granting completion approval.".into(),
                TicketSource::Human,
                ActorRef::human("test-reviewer"),
                ApprovalPolicy::HumanBeforeCompletion,
            )
            .await
            .unwrap();
        store
            .add_run(
                &ticket.id,
                ActorRef::system(),
                Run {
                    id: RunId::new(),
                    ticket_id: ticket.id.clone(),
                    loom_session_id: None,
                    status: RunStatus::Succeeded,
                    evidence: Some(RunEvidence {
                        summary: "Synthetic passing result".into(),
                        commands: vec![],
                        artifacts: vec![],
                        risks: vec![],
                    }),
                },
            )
            .await
            .unwrap();
        let expected = if accept_first {
            store
                .accept_ticket(&ticket.id, ActorRef::human("test-reviewer"))
                .await
                .unwrap();
            TicketStatus::Accepted
        } else {
            TicketStatus::Completed
        };
        assert_eq!(store.get_ticket(&ticket.id).await.unwrap().status, expected);
        assert!(!store.has_approval(&ticket.id).await.unwrap());
        assert!(matches!(
            store.close_ticket(&ticket.id, ActorRef::system()).await,
            Err(StoreError::ApprovalRequired)
        ));
        for _ in 0..2 {
            let approved = store
                .grant_approval(&ticket.id, ActorRef::human("test-reviewer"))
                .await
                .unwrap();
            assert_eq!(approved.status, expected);
            assert!(store.has_approval(&ticket.id).await.unwrap());
        }
        reviews.push((ticket.id, expected));
    }
    reviews
}

async fn finish_reviews(store: &impl TicketStore, reviews: Vec<(TicketId, TicketStatus)>) {
    for (id, expected) in reviews {
        assert_eq!(store.get_ticket(&id).await.unwrap().status, expected);
        assert!(store.has_approval(&id).await.unwrap());
        let closed = store
            .close_ticket(&id, ActorRef::human("test-reviewer"))
            .await
            .unwrap();
        assert_eq!(closed.status, TicketStatus::Closed);
        assert!(matches!(
            store.grant_approval(&id, ActorRef::system()).await,
            Err(StoreError::InvalidTransition(_))
        ));
    }
    // Normal pre-execution approval still advances Open to Approved, and a
    // later cancellation cannot be undone by another approval request.
    let open = store
        .create_ticket(
            "Synthetic pre-execution approval".into(),
            "No execution required.".into(),
            TicketSource::Human,
            ActorRef::system(),
        )
        .await
        .unwrap();
    let approved = store
        .grant_approval(&open.id, ActorRef::system())
        .await
        .unwrap();
    assert_eq!(approved.status, TicketStatus::Approved);
    store
        .cancel_ticket(&open.id, ActorRef::system())
        .await
        .unwrap();
    assert!(matches!(
        store.grant_approval(&open.id, ActorRef::system()).await,
        Err(StoreError::InvalidTransition(_))
    ));
}

#[tokio::test]
async fn memory_completion_approval_preserves_review_milestones() {
    let store = InMemoryTicketStore::default();
    let reviews = prepare_completion_reviews(&store).await;
    finish_reviews(&store, reviews).await;
}

#[tokio::test]
async fn sqlite_completion_approval_preserves_review_milestones_across_reopen() {
    let path = std::env::temp_dir().join(format!("tea-accepted-approval-{}.sqlite", RunId::new()));
    let store = SqliteTicketStore::open(&path).unwrap();
    let reviews = prepare_completion_reviews(&store).await;
    drop(store);
    let reopened = SqliteTicketStore::open(&path).unwrap();
    finish_reviews(&reopened, reviews).await;
    drop(reopened);
    let _ = std::fs::remove_file(path);
}
