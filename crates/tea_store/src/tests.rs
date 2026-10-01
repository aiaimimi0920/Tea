use super::*;
use rusqlite::{params, Connection, TransactionBehavior};
use std::sync::Arc;
use tea_core::{
    ApprovalPolicy, RiskLevel, RunEvidence, RunStatus, TicketAnalysis, TicketEventKind,
};

#[tokio::test]
async fn create_ticket_appends_created_event() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Smoke".to_string(),
            "Create a safe plan.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();

    let events = store.ticket_events(&created.id).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, TicketEventKind::TicketCreated);
    assert_eq!(events[0].actor, ActorRef::human("vmjcv"));
}

#[tokio::test]
async fn memory_idempotent_create_replays_and_rejects_changed_requests() {
    let store = InMemoryTicketStore::default();
    let idempotency = IdempotencyRequest::new("human-ticket-create", "request-1", "hash-1");
    let create = |request: IdempotencyRequest| {
        store.create_ticket_idempotent(
            "Idempotent memory ticket".to_string(),
            "The same logical request must create one ticket.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::HumanBeforeExecute,
            TicketCreateOptions::default(),
            Some(request),
        )
    };

    let first = create(idempotency.clone()).await.unwrap();
    let replay = create(idempotency).await.unwrap();

    assert_eq!(replay, first);
    assert_eq!(store.list_tickets().await.unwrap().len(), 1);
    assert_eq!(store.ticket_events(&first.id).await.unwrap().len(), 1);

    let conflict = create(IdempotencyRequest::new(
        "human-ticket-create",
        "request-1",
        "hash-2",
    ))
    .await
    .unwrap_err();
    assert!(matches!(conflict, StoreError::IdempotencyConflict));
}

#[tokio::test]
async fn sqlite_idempotent_create_survives_reopen_and_replays_original_response() {
    let path = temp_store_path("tea-store-idempotency-reopen");
    let request = IdempotencyRequest::new("human-ticket-create", "request-1", "hash-1");
    let first = {
        let store = SqliteTicketStore::open(&path).unwrap();
        let first = store
            .create_ticket_idempotent(
                "Idempotent SQLite ticket".to_string(),
                "The persisted response must survive a daemon restart.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
                ApprovalPolicy::HumanBeforeExecute,
                TicketCreateOptions::default(),
                Some(request.clone()),
            )
            .await
            .unwrap();
        store
            .update_ticket_fields(
                &first.id,
                ActorRef::human("vmjcv"),
                TicketEdits {
                    title: Some("Edited after creation".to_string()),
                    ..TicketEdits::default()
                },
            )
            .await
            .unwrap();
        first
    };

    let reopened = SqliteTicketStore::open(&path).unwrap();
    let replay = reopened
        .create_ticket_idempotent(
            "Idempotent SQLite ticket".to_string(),
            "The persisted response must survive a daemon restart.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::HumanBeforeExecute,
            TicketCreateOptions::default(),
            Some(request),
        )
        .await
        .unwrap();

    assert_eq!(replay, first);
    assert_eq!(reopened.list_tickets().await.unwrap().len(), 1);
    assert_eq!(
        reopened.get_ticket(&first.id).await.unwrap().title,
        "Edited after creation"
    );
    {
        let conn = reopened.conn.lock().unwrap();
        let key_count = conn
            .query_row("SELECT COUNT(*) FROM idempotency_keys", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(key_count, 1);
    }
    let status = reopened.store_status().await.unwrap();
    assert_eq!(status.idempotency_key_count, Some(1));
    assert!(status.sqlite_page_count.is_some_and(|count| count > 0));
    assert!(status.sqlite_freelist_count.is_some());
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn sqlite_concurrent_idempotent_creates_across_connections_create_one_ticket() {
    let path = temp_store_path("tea-store-idempotency-concurrent");
    let first_store = SqliteTicketStore::open(&path).unwrap();
    let second_store = SqliteTicketStore::open(&path).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let create = |store: SqliteTicketStore, barrier: Arc<std::sync::Barrier>| {
        std::thread::spawn(move || {
            barrier.wait();
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(store.create_ticket_idempotent(
                    "Concurrent idempotent ticket".to_string(),
                    "Independent SQLite connections must converge on one response.".to_string(),
                    TicketSource::Human,
                    ActorRef::human("vmjcv"),
                    ApprovalPolicy::HumanBeforeExecute,
                    TicketCreateOptions::default(),
                    Some(IdempotencyRequest::new(
                        "human-ticket-create",
                        "concurrent-request",
                        "concurrent-hash",
                    )),
                ))
        })
    };
    let first = create(first_store, barrier.clone());
    let second = create(second_store, barrier.clone());
    barrier.wait();

    let first = first.join().unwrap().unwrap();
    let second = second.join().unwrap().unwrap();

    assert_eq!(first, second);
    let reopened = SqliteTicketStore::open(&path).unwrap();
    assert_eq!(reopened.list_tickets().await.unwrap().len(), 1);
    assert_eq!(reopened.ticket_events(&first.id).await.unwrap().len(), 1);
    drop(reopened);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn listing_tickets_is_deterministic() {
    let store = InMemoryTicketStore::default();
    store
        .create_ticket(
            "A".to_string(),
            "first".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .create_ticket(
            "B".to_string(),
            "second".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();

    let tickets = store.list_tickets().await.unwrap();
    assert_eq!(
        tickets
            .iter()
            .map(|ticket| ticket.title.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B"]
    );
}

async fn assert_ticket_pagination_scenario(store: &impl TicketStore) {
    let mut created = Vec::new();
    for (title, source) in [
        ("A", TicketSource::Human),
        ("B", TicketSource::Hook),
        ("C", TicketSource::Human),
        ("D", TicketSource::Hook),
        ("E", TicketSource::Human),
    ] {
        created.push(
            store
                .create_ticket(
                    title.to_string(),
                    format!("ticket {title}"),
                    source,
                    ActorRef::human("vmjcv"),
                )
                .await
                .unwrap(),
        );
    }
    store
        .add_comment(
            &created[2].id,
            ActorRef::human("vmjcv"),
            "page metric".to_string(),
        )
        .await
        .unwrap();

    let first = store
        .list_tickets_page(TicketPageRequest {
            after_ordinal: None,
            limit: 2,
            status: None,
            source: None,
        })
        .await
        .unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|ticket| ticket.title.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B"]
    );
    assert_eq!(first.next_ordinal, Some(1));

    let second = store
        .list_tickets_page(TicketPageRequest {
            after_ordinal: first.next_ordinal,
            limit: 2,
            status: None,
            source: None,
        })
        .await
        .unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|ticket| ticket.title.as_str())
            .collect::<Vec<_>>(),
        vec!["C", "D"]
    );
    assert_eq!(second.next_ordinal, Some(3));

    let final_page = store
        .list_tickets_page(TicketPageRequest {
            after_ordinal: second.next_ordinal,
            limit: 2,
            status: None,
            source: None,
        })
        .await
        .unwrap();
    assert_eq!(final_page.items.len(), 1);
    assert_eq!(final_page.items[0].title, "E");
    assert_eq!(final_page.next_ordinal, None);

    let hook_page = store
        .list_tickets_page(TicketPageRequest {
            after_ordinal: None,
            limit: 1,
            status: None,
            source: Some(TicketSource::Hook),
        })
        .await
        .unwrap();
    assert_eq!(hook_page.items[0].title, "B");
    assert_eq!(hook_page.next_ordinal, Some(1));
    let hook_final = store
        .list_tickets_page(TicketPageRequest {
            after_ordinal: hook_page.next_ordinal,
            limit: 1,
            status: None,
            source: Some(TicketSource::Hook),
        })
        .await
        .unwrap();
    assert_eq!(hook_final.items[0].title, "D");
    assert_eq!(hook_final.next_ordinal, None);

    let metrics = store
        .ticket_metrics_page(TicketPageRequest {
            after_ordinal: Some(1),
            limit: 2,
            status: Some(TicketStatus::Open),
            source: None,
        })
        .await
        .unwrap();
    assert_eq!(metrics.items.len(), 2);
    assert_eq!(metrics.items[0].ticket_id, created[2].id);
    assert_eq!(metrics.items[0].comments_count, 1);
    assert_eq!(metrics.items[1].ticket_id, created[3].id);
    assert_eq!(metrics.next_ordinal, Some(3));

    let invalid = store
        .list_tickets_page(TicketPageRequest {
            after_ordinal: None,
            limit: 0,
            status: None,
            source: None,
        })
        .await;
    assert!(matches!(invalid, Err(StoreError::InvalidPageRequest(_))));
}

#[tokio::test]
async fn ticket_pagination_is_consistent_in_memory() {
    assert_ticket_pagination_scenario(&InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_ticket_pagination_uses_stable_ordinals() {
    let path = temp_store_path("tea-store-sqlite-ticket-pagination");
    let store = SqliteTicketStore::open(&path).unwrap();
    assert_ticket_pagination_scenario(&store).await;

    let conn = store.conn.lock().unwrap();
    let explain_list_plan = |filter_status: bool, filter_source: bool| {
        let sql = sqlite_list_tickets_page_sql(filter_status, filter_source);
        let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let after = Some(1_i64);
        let status = filter_status.then(|| "open".to_string());
        let source = filter_source.then(|| "human".to_string());
        let limit = 3_i64;
        let binds = ticket_page_binds(&after, &status, &source, &limit);
        statement
            .query_map(binds.as_slice(), |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };

    let plan = explain_list_plan(false, false);
    assert!(
        plan.iter()
            .any(|detail| detail.contains("uidx_tickets_ordinal")),
        "query plan did not use the unique ticket ordinal index: {plan:?}"
    );

    // Filtered pages must search the v4 generated-column indexes rather
    // than json-extracting every row along the ordinal index.
    let status_plan = explain_list_plan(true, false);
    assert!(
        status_plan
            .iter()
            .any(|detail| detail.contains("USING INDEX idx_tickets_status_ordinal")),
        "status-filtered list plan did not use the status index: {status_plan:?}"
    );
    let source_plan = explain_list_plan(false, true);
    assert!(
        source_plan
            .iter()
            .any(|detail| detail.contains("USING INDEX idx_tickets_source_ordinal")),
        "source-filtered list plan did not use the source index: {source_plan:?}"
    );
    let combined_plan = explain_list_plan(true, true);
    assert!(
        combined_plan.iter().any(|detail| {
            detail.contains("USING INDEX idx_tickets_status_ordinal")
                || detail.contains("USING INDEX idx_tickets_source_ordinal")
        }),
        "combined-filter list plan did not use a filter index: {combined_plan:?}"
    );
    for plan in [&status_plan, &source_plan, &combined_plan] {
        assert!(
            !plan.iter().any(|detail| detail.starts_with("SCAN")),
            "filtered list plan scanned the tickets table: {plan:?}"
        );
    }
}

#[test]
fn sqlite_ticket_metrics_page_uses_ticket_scoped_indexes() {
    let path = temp_store_path("tea-store-sqlite-ticket-metrics-plan");
    let store = SqliteTicketStore::open(&path).unwrap();
    let conn = store.conn.lock().unwrap();
    let explain_metrics_plan = |filter_status: bool, filter_source: bool| {
        let sql = sqlite_ticket_metrics_page_sql(filter_status, filter_source);
        let mut statement = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
        let after = Some(1_i64);
        let status = filter_status.then(|| "open".to_string());
        let source = filter_source.then(|| "human".to_string());
        let limit = 201_i64;
        let binds = ticket_page_binds(&after, &status, &source, &limit);
        statement
            .query_map(binds.as_slice(), |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let plan = explain_metrics_plan(false, false);

    for index in [
        "uidx_tickets_ordinal",
        "uidx_comments_ticket_ordinal",
        "uidx_runs_ticket_ordinal",
        "uidx_events_ticket_ordinal",
    ] {
        assert!(
            plan.iter().any(|detail| detail.contains(index)),
            "metrics query plan did not use {index}: {plan:?}"
        );
    }
    assert!(
        plan.iter()
            .filter(|detail| detail.contains("uidx_comments_ticket_ordinal"))
            .count()
            >= 2,
        "metrics query plan did not index both comment lookups: {plan:?}"
    );
    assert!(
        !plan.iter().any(|detail| {
            detail.starts_with("SCAN c")
                || detail.starts_with("SCAN r")
                || detail.starts_with("SCAN e")
        }),
        "metrics query plan scanned a complete child table: {plan:?}"
    );

    // Filtered metrics pages must search the v4 generated-column indexes
    // rather than json-extracting every row along the ordinal index.
    let status_plan = explain_metrics_plan(true, false);
    assert!(
        status_plan
            .iter()
            .any(|detail| detail.contains("USING INDEX idx_tickets_status_ordinal")),
        "status-filtered metrics plan did not use the status index: {status_plan:?}"
    );
    let source_plan = explain_metrics_plan(false, true);
    assert!(
        source_plan
            .iter()
            .any(|detail| detail.contains("USING INDEX idx_tickets_source_ordinal")),
        "source-filtered metrics plan did not use the source index: {source_plan:?}"
    );
    let combined_plan = explain_metrics_plan(true, true);
    assert!(
        combined_plan.iter().any(|detail| {
            detail.contains("USING INDEX idx_tickets_status_ordinal")
                || detail.contains("USING INDEX idx_tickets_source_ordinal")
        }),
        "combined-filter metrics plan did not use a filter index: {combined_plan:?}"
    );
    for plan in [&status_plan, &source_plan, &combined_plan] {
        assert!(
            !plan.iter().any(|detail| detail.starts_with("SCAN t")),
            "filtered metrics plan scanned the tickets table: {plan:?}"
        );
    }

    drop(conn);
    drop(store);
    let mut wal_path = path.as_os_str().to_os_string();
    wal_path.push("-wal");
    let mut shm_path = path.as_os_str().to_os_string();
    shm_path.push("-shm");
    for candidate in [path, wal_path.into(), shm_path.into()] {
        let _ = std::fs::remove_file(candidate);
    }
}

async fn assert_ticket_bundle_scenario(store: &impl TicketStore) {
    let ticket = store
        .create_ticket(
            "Bundle".to_string(),
            "body".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .add_comment(&ticket.id, ActorRef::human("vmjcv"), "note".to_string())
        .await
        .unwrap();
    store
        .add_run(&ticket.id, ActorRef::system(), successful_run(&ticket.id))
        .await
        .unwrap();

    let bundle = store.ticket_bundle(&ticket.id).await.unwrap();
    assert_eq!(bundle.ticket.id, ticket.id);
    assert_eq!(bundle.comments.len(), 1);
    assert_eq!(bundle.comments[0].body, "note");
    assert_eq!(bundle.runs.len(), 1);
    assert!(!bundle.events.is_empty());
    assert!(bundle.analysis.is_none());
    assert!(bundle.plan.is_none());

    let missing = store.ticket_bundle(&TicketId::new()).await;
    assert!(matches!(missing, Err(StoreError::TicketNotFound)));
}

#[tokio::test]
async fn ticket_bundle_reads_detail_in_memory() {
    let store = InMemoryTicketStore::default();
    assert_ticket_bundle_scenario(&store).await;
}

#[tokio::test]
async fn sqlite_ticket_bundle_reads_detail() {
    let path = temp_store_path("tea-store-sqlite-ticket-bundle");
    let store = SqliteTicketStore::open(&path).unwrap();
    assert_ticket_bundle_scenario(&store).await;
}

async fn assert_ticket_metrics_scenario(store: &impl TicketStore) {
    let a = store
        .create_ticket(
            "A".to_string(),
            "first".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let b = store
        .create_ticket(
            "B".to_string(),
            "second".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .add_comment(&a.id, ActorRef::human("vmjcv"), "hello".to_string())
        .await
        .unwrap();
    store
        .add_comment(&a.id, ActorRef::human("vmjcv"), "world".to_string())
        .await
        .unwrap();
    store
        .add_run(&a.id, ActorRef::system(), successful_run(&a.id))
        .await
        .unwrap();

    let metrics = store.ticket_metrics().await.unwrap();
    assert_eq!(metrics.len(), 2);

    // Metrics follow list_tickets order (A, then B).
    let first = &metrics[0];
    assert_eq!(first.ticket_id, a.id);
    assert_eq!(first.comments_count, 2);
    assert_eq!(first.runs_count, 1);
    assert_eq!(first.latest_comment.as_ref().unwrap().body, "world");
    assert!(first.latest_event.is_some());

    let second = &metrics[1];
    assert_eq!(second.ticket_id, b.id);
    assert_eq!(second.comments_count, 0);
    assert_eq!(second.runs_count, 0);
    assert!(second.latest_comment.is_none());
}

#[tokio::test]
async fn ticket_metrics_aggregates_counts_and_latest_in_memory() {
    let store = InMemoryTicketStore::default();
    assert_ticket_metrics_scenario(&store).await;
}

#[tokio::test]
async fn sqlite_ticket_metrics_aggregates_counts_and_latest() {
    let path = temp_store_path("tea-store-sqlite-ticket-metrics");
    let store = SqliteTicketStore::open(&path).unwrap();
    assert_ticket_metrics_scenario(&store).await;
}

#[tokio::test]
async fn set_analysis_appends_event_and_updates_policy() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Smoke".to_string(),
            "Create a safe plan.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .set_analysis(
            &created.id,
            ActorRef::system(),
            TicketAnalysis {
                intent: "engineering".to_string(),
                target_components: vec!["Tea".to_string()],
                target_paths: vec![],
                constraints: vec![],
                acceptance_criteria: vec!["tests pass".to_string()],
                missing_context: vec![],
                risk_assessment: RiskLevel::Low,
                confidence: 0.8,
                recommended_policy: ApprovalPolicy::HumanBeforeExecute,
                recommended_workflow: "mock".to_string(),
            },
        )
        .await
        .unwrap();

    let events = store.ticket_events(&created.id).await.unwrap();
    assert_eq!(events.last().unwrap().kind, TicketEventKind::TicketAnalyzed);
    let ticket = store.get_ticket(&created.id).await.unwrap();
    assert_eq!(ticket.status, TicketStatus::AnalysisReady);
    assert_eq!(ticket.risk_level, RiskLevel::Low);
    let stored = store.ticket_analysis(&created.id).await.unwrap().unwrap();
    assert_eq!(stored.intent, "engineering");
}

#[tokio::test]
async fn set_plan_can_be_read_back_for_audit_export() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Smoke".to_string(),
            "Create a safe plan.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .set_plan(
            &created.id,
            ActorRef::system(),
            Plan {
                summary: "Use the stored plan in audit export.".to_string(),
                steps: vec![tea_core::PlanStep {
                    id: "audit".to_string(),
                    title: "Audit".to_string(),
                    description: "Read plan back through the store.".to_string(),
                }],
                required_tools: vec![],
                expected_artifacts: vec![],
                validation_strategy: vec![],
                rollback_strategy: vec![],
                requires_approval_before_execute: true,
            },
        )
        .await
        .unwrap();

    let stored = store.ticket_plan(&created.id).await.unwrap().unwrap();
    assert_eq!(stored.summary, "Use the stored plan in audit export.");
}

async fn assert_missing_context_plan_stays_blocked<S: TicketStore>(store: &S) {
    let created = store
        .create_ticket_with_policy(
            "Need context".to_string(),
            "Do not execute until the missing context is supplied.".to_string(),
            TicketSource::Human,
            ActorRef::human("reviewer"),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    store
        .set_analysis(
            &created.id,
            ActorRef::system(),
            TicketAnalysis {
                intent: "collect context".to_string(),
                target_components: vec!["Tea".to_string()],
                target_paths: vec![],
                constraints: vec![],
                acceptance_criteria: vec!["context is supplied".to_string()],
                missing_context: vec!["target repository".to_string()],
                risk_assessment: RiskLevel::Low,
                confidence: 0.2,
                recommended_policy: ApprovalPolicy::AlwaysAuto,
                recommended_workflow: "wait-for-context".to_string(),
            },
        )
        .await
        .unwrap();
    store
        .set_plan(
            &created.id,
            ActorRef::system(),
            Plan {
                summary: "Resume after the missing context is supplied.".to_string(),
                steps: vec![tea_core::PlanStep {
                    id: "wait".to_string(),
                    title: "Wait".to_string(),
                    description: "Collect the required context.".to_string(),
                }],
                required_tools: vec![],
                expected_artifacts: vec![],
                validation_strategy: vec![],
                rollback_strategy: vec![],
                requires_approval_before_execute: false,
            },
        )
        .await
        .unwrap();

    assert_eq!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::NeedsInfo
    );
    let error = store
        .add_run(
            &created.id,
            ActorRef::loom("test"),
            Run {
                id: RunId::new(),
                ticket_id: created.id.clone(),
                loom_session_id: None,
                status: RunStatus::Queued,
                evidence: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidTransition(_)));
}

#[tokio::test]
async fn in_memory_missing_context_plan_stays_blocked() {
    assert_missing_context_plan_stays_blocked(&InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_missing_context_plan_stays_blocked() {
    let path = temp_store_path("tea-store-sqlite-needs-info-plan");
    let store = SqliteTicketStore::open(&path).unwrap();
    assert_missing_context_plan_stays_blocked(&store).await;
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn update_ticket_fields_edits_and_appends_event() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Original title".to_string(),
            "Original description.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();

    let updated = store
        .update_ticket_fields(
            &created.id,
            ActorRef::human("vmjcv"),
            TicketEdits {
                title: Some("Edited title".to_string()),
                description: None,
                priority: Some("high".to_string()),
                labels: Some(vec!["area:auth".to_string()]),
            },
        )
        .await
        .unwrap();

    assert_eq!(updated.title, "Edited title");
    assert_eq!(updated.priority, "high");
    assert!(updated.labels.contains(&"area:auth".to_string()));
    // System-derived labels survive the edit.
    assert!(updated.labels.iter().any(|label| label == "source:human"));
    assert!(updated
        .labels
        .iter()
        .any(|label| label.starts_with("policy:")));

    let events = store.ticket_events(&created.id).await.unwrap();
    assert_eq!(events.last().unwrap().kind, TicketEventKind::TicketEdited);
}

#[tokio::test]
async fn update_ticket_fields_rejects_terminal_ticket() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Terminal".to_string(),
            "Will be cancelled before an edit is attempted.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .cancel_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();

    let result = store
        .update_ticket_fields(
            &created.id,
            ActorRef::human("vmjcv"),
            TicketEdits {
                title: Some("Too late".to_string()),
                description: None,
                priority: None,
                labels: None,
            },
        )
        .await;

    assert!(matches!(result, Err(StoreError::InvalidTransition(_))));
}

#[tokio::test]
async fn ticket_mutations_advance_updated_at() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Freshness".to_string(),
            "Ticket mutations should be visible to UI sorting.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(5));
    let approved = store
        .grant_approval(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();

    assert!(approved.updated_at > created.updated_at);
    let stored = store.get_ticket(&created.id).await.unwrap();
    assert_eq!(stored.updated_at, approved.updated_at);
}

#[tokio::test]
async fn sqlite_ticket_mutations_persist_updated_at() {
    let path = temp_store_path("tea-store-sqlite-updated-at");
    let store = SqliteTicketStore::open(&path).unwrap();
    let created = store
        .create_ticket(
            "Freshness".to_string(),
            "Persist updated_at after ticket mutations.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(5));
    let approved = store
        .grant_approval(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();
    assert!(approved.updated_at > created.updated_at);

    let reopened = SqliteTicketStore::open(&path).unwrap();
    let stored = reopened.get_ticket(&created.id).await.unwrap();
    assert_eq!(stored.updated_at, approved.updated_at);

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn update_run_rejects_ticket_mismatch() {
    let store = InMemoryTicketStore::default();
    let owner = store
        .create_ticket(
            "Owner".to_string(),
            "Run belongs to this ticket.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let other = store
        .create_ticket(
            "Other".to_string(),
            "This ticket must not receive another ticket's run event.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let run = successful_run(&owner.id);
    store
        .add_run(&owner.id, ActorRef::system(), run.clone())
        .await
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut stopped = run.clone();
    stopped.status = RunStatus::Stopped;
    let err = store
        .update_run(&other.id, ActorRef::loom("test-loom"), stopped)
        .await
        .unwrap_err();

    assert!(matches!(err, StoreError::InvalidRunTransition(_)));
    assert_eq!(store.get_run(&run.id).await.unwrap().status, run.status);
    assert_eq!(
        store.get_ticket(&other.id).await.unwrap().updated_at,
        other.updated_at
    );
}

#[tokio::test]
async fn sqlite_update_run_rejects_ticket_mismatch() {
    let path = temp_store_path("tea-store-sqlite-run-ticket-mismatch");
    let store = SqliteTicketStore::open(&path).unwrap();
    let owner = store
        .create_ticket(
            "Owner".to_string(),
            "Run belongs to this ticket.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let other = store
        .create_ticket(
            "Other".to_string(),
            "This ticket must not receive another ticket's run event.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let run = successful_run(&owner.id);
    store
        .add_run(&owner.id, ActorRef::system(), run.clone())
        .await
        .unwrap();

    std::thread::sleep(std::time::Duration::from_millis(5));
    let mut stopped = run.clone();
    stopped.status = RunStatus::Stopped;
    let err = store
        .update_run(&other.id, ActorRef::loom("test-loom"), stopped)
        .await
        .unwrap_err();

    assert!(matches!(err, StoreError::InvalidRunTransition(_)));
    assert_eq!(store.get_run(&run.id).await.unwrap().status, run.status);
    assert_eq!(
        store.get_ticket(&other.id).await.unwrap().updated_at,
        other.updated_at
    );

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn update_run_persists_evidence_and_terminal_ticket_state() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Runtime update".to_string(),
            "Persist a later Loom completion response.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let running = running_run(&created.id);
    store
        .add_run(&created.id, ActorRef::loom("test-loom"), running.clone())
        .await
        .unwrap();

    let mut succeeded = successful_run(&created.id);
    succeeded.id = running.id.clone();
    succeeded.loom_session_id = running.loom_session_id.clone();
    let updated = store
        .update_run(&created.id, ActorRef::loom("test-loom"), succeeded.clone())
        .await
        .unwrap();

    assert_eq!(updated, succeeded);
    assert_eq!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::Completed
    );
    let events = store.ticket_events(&created.id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.kind == TicketEventKind::RunSucceeded));
    assert_eq!(
        events.last().unwrap().kind,
        TicketEventKind::EvidenceAttached
    );
}

#[tokio::test]
async fn sqlite_update_run_persists_evidence_and_terminal_ticket_state() {
    let path = temp_store_path("tea-store-sqlite-run-completion");
    let store = SqliteTicketStore::open(&path).unwrap();
    let created = store
        .create_ticket(
            "Runtime update".to_string(),
            "Persist a later Loom completion response.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let running = running_run(&created.id);
    store
        .add_run(&created.id, ActorRef::loom("test-loom"), running.clone())
        .await
        .unwrap();

    let mut succeeded = successful_run(&created.id);
    succeeded.id = running.id.clone();
    succeeded.loom_session_id = running.loom_session_id.clone();
    store
        .update_run(&created.id, ActorRef::loom("test-loom"), succeeded.clone())
        .await
        .unwrap();

    assert_eq!(store.get_run(&running.id).await.unwrap(), succeeded);
    assert_eq!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::Completed
    );
    let events = store.ticket_events(&created.id).await.unwrap();
    assert_eq!(
        events.last().unwrap().kind,
        TicketEventKind::EvidenceAttached
    );

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn historical_run_update_does_not_regress_memory_ticket_status() {
    let store = InMemoryTicketStore::default();
    assert_historical_run_update_preserves_latest_ticket_status(&store).await;
}

#[tokio::test]
async fn historical_run_update_does_not_regress_sqlite_ticket_status() {
    let path = temp_store_path("tea-store-sqlite-historical-run-update");
    let store = SqliteTicketStore::open(&path).unwrap();
    assert_historical_run_update_preserves_latest_ticket_status(&store).await;

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn duplicate_run_ids_are_rejected_consistently() {
    let memory = InMemoryTicketStore::default();
    let memory_ticket = memory
        .create_ticket(
            "Duplicate run".to_string(),
            "Reject duplicate run identifiers.".to_string(),
            TicketSource::Human,
            ActorRef::system(),
        )
        .await
        .unwrap();
    let run = successful_run(&memory_ticket.id);
    memory
        .add_run(&memory_ticket.id, ActorRef::system(), run.clone())
        .await
        .unwrap();
    assert!(matches!(
        memory
            .add_run(&memory_ticket.id, ActorRef::system(), run.clone())
            .await
            .unwrap_err(),
        StoreError::InvalidRunTransition(_)
    ));

    let path = temp_store_path("tea-store-sqlite-duplicate-run");
    let sqlite = SqliteTicketStore::open(&path).unwrap();
    let sqlite_ticket = sqlite
        .create_ticket(
            "Duplicate run".to_string(),
            "Reject duplicate run identifiers.".to_string(),
            TicketSource::Human,
            ActorRef::system(),
        )
        .await
        .unwrap();
    let mut sqlite_run = run;
    sqlite_run.ticket_id = sqlite_ticket.id.clone();
    sqlite
        .add_run(&sqlite_ticket.id, ActorRef::system(), sqlite_run.clone())
        .await
        .unwrap();
    assert!(matches!(
        sqlite
            .add_run(&sqlite_ticket.id, ActorRef::system(), sqlite_run)
            .await
            .unwrap_err(),
        StoreError::InvalidRunTransition(_)
    ));

    let _ = std::fs::remove_file(path);
}

async fn assert_initial_run_status_semantics(store: &impl TicketStore) {
    let queued_ticket = store
        .create_ticket(
            "Queued run".to_string(),
            "Keep queued work distinct from work that has started.".to_string(),
            TicketSource::Human,
            ActorRef::system(),
        )
        .await
        .unwrap();
    let queued_run = Run {
        id: RunId::new(),
        ticket_id: queued_ticket.id.clone(),
        loom_session_id: Some("queued-session".to_string()),
        status: RunStatus::Queued,
        evidence: None,
    };
    store
        .add_run(&queued_ticket.id, ActorRef::system(), queued_run)
        .await
        .unwrap();

    let queued_events = store.ticket_events(&queued_ticket.id).await.unwrap();
    assert!(queued_events
        .iter()
        .any(|event| event.kind == TicketEventKind::RunQueued));
    assert!(!queued_events
        .iter()
        .any(|event| event.kind == TicketEventKind::RunStarted));
    assert_eq!(
        store.get_ticket(&queued_ticket.id).await.unwrap().status,
        TicketStatus::Running
    );

    let stopped_ticket = store
        .create_ticket(
            "Stopped run".to_string(),
            "Reflect an immediately stopped Loom run in ticket state.".to_string(),
            TicketSource::Human,
            ActorRef::system(),
        )
        .await
        .unwrap();
    let stopped_run = Run {
        id: RunId::new(),
        ticket_id: stopped_ticket.id.clone(),
        loom_session_id: Some("stopped-session".to_string()),
        status: RunStatus::Stopped,
        evidence: None,
    };
    store
        .add_run(&stopped_ticket.id, ActorRef::system(), stopped_run)
        .await
        .unwrap();

    let stopped_events = store.ticket_events(&stopped_ticket.id).await.unwrap();
    assert!(stopped_events
        .iter()
        .any(|event| event.kind == TicketEventKind::RunStarted));
    assert_eq!(
        store.get_ticket(&stopped_ticket.id).await.unwrap().status,
        TicketStatus::NeedsReview
    );
}

#[tokio::test]
async fn initial_run_status_controls_memory_ticket_state_and_events() {
    assert_initial_run_status_semantics(&InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn initial_run_status_controls_sqlite_ticket_state_and_events() {
    let path = temp_store_path("tea-store-sqlite-initial-run-status");
    let store = SqliteTicketStore::open(&path).unwrap();

    assert_initial_run_status_semantics(&store).await;

    let _ = std::fs::remove_file(path);
}

async fn assert_run_transition_guards_and_audit_events(store: &impl TicketStore) {
    let ticket = store
        .create_ticket_with_policy(
            "Run lifecycle".to_string(),
            "Preserve terminal outcomes and audit operator run actions.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    let running = running_run(&ticket.id);
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), running.clone())
        .await
        .unwrap();

    let mut stopped = running.clone();
    stopped.status = RunStatus::Stopped;
    store
        .update_run(&ticket.id, ActorRef::loom("test-loom"), stopped.clone())
        .await
        .unwrap();

    let mut retrying = stopped;
    retrying.status = RunStatus::Retrying;
    store
        .update_run(&ticket.id, ActorRef::loom("test-loom"), retrying.clone())
        .await
        .unwrap();

    let mut succeeded = successful_run(&ticket.id);
    succeeded.id = retrying.id.clone();
    succeeded.loom_session_id = retrying.loom_session_id.clone();
    store
        .update_run(&ticket.id, ActorRef::loom("test-loom"), succeeded.clone())
        .await
        .unwrap();

    let mut invalid_stop = succeeded.clone();
    invalid_stop.status = RunStatus::Stopped;
    let error = store
        .update_run(&ticket.id, ActorRef::loom("test-loom"), invalid_stop)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidRunTransition(_)));
    assert_eq!(store.get_run(&succeeded.id).await.unwrap(), succeeded);

    let mut evidence_downgrade = succeeded.clone();
    evidence_downgrade.evidence = None;
    let error = store
        .update_run(&ticket.id, ActorRef::loom("test-loom"), evidence_downgrade)
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidRunTransition(_)));
    assert_eq!(store.get_run(&succeeded.id).await.unwrap(), succeeded);

    let events = store.ticket_events(&ticket.id).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == TicketEventKind::RunStopped)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == TicketEventKind::RunRetrying)
            .count(),
        1
    );
}

#[tokio::test]
async fn memory_run_transitions_are_guarded_and_audited() {
    assert_run_transition_guards_and_audit_events(&InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_run_transitions_are_guarded_and_audited() {
    let path = temp_store_path("tea-store-sqlite-run-transition-guard");
    let store = SqliteTicketStore::open(&path).unwrap();

    assert_run_transition_guards_and_audit_events(&store).await;

    drop(store);
    let _ = std::fs::remove_file(path);
}

async fn assert_run_compare_and_set_rejects_stale_updates(store: &impl TicketStore) {
    let ticket = store
        .create_ticket(
            "Concurrent run action".to_string(),
            "Do not persist a Loom response based on stale run state.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let running = running_run(&ticket.id);
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), running.clone())
        .await
        .unwrap();

    let mut first_stop = running.clone();
    first_stop.status = RunStatus::Stopped;
    store
        .update_run(
            &ticket.id,
            ActorRef::loom("first-action"),
            first_stop.clone(),
        )
        .await
        .unwrap();
    let events_before_stale_update = store.ticket_events(&ticket.id).await.unwrap();

    let mut stale_stop = running.clone();
    stale_stop.status = RunStatus::Stopped;
    stale_stop.loom_session_id = Some("stale-response".to_string());
    let error = store
        .update_run_if_unchanged(
            &ticket.id,
            ActorRef::loom("stale-action"),
            running,
            stale_stop,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::RunConflict(_)));
    assert_eq!(store.get_run(&first_stop.id).await.unwrap(), first_stop);
    assert_eq!(
        store.ticket_events(&ticket.id).await.unwrap(),
        events_before_stale_update
    );

    let newer = running_run(&ticket.id);
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), newer)
        .await
        .unwrap();
    let mut retrying_old_run = first_stop.clone();
    retrying_old_run.status = RunStatus::Retrying;
    let error = store
        .update_latest_run_if_unchanged(
            &ticket.id,
            ActorRef::loom("stale-latest-action"),
            first_stop.clone(),
            retrying_old_run,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::RunConflict(_)));
    assert_eq!(store.get_run(&first_stop.id).await.unwrap(), first_stop);
}

#[tokio::test]
async fn memory_run_compare_and_set_rejects_stale_updates() {
    assert_run_compare_and_set_rejects_stale_updates(&InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_run_compare_and_set_rejects_stale_updates() {
    let path = temp_store_path("tea-store-sqlite-run-cas");
    let store = SqliteTicketStore::open(&path).unwrap();

    assert_run_compare_and_set_rejects_stale_updates(&store).await;

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn failed_ticket_with_historical_evidence_cannot_be_accepted() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Failed review".to_string(),
            "Do not accept evidence from a failed execution.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let mut failed = successful_run(&created.id);
    failed.status = RunStatus::Failed;
    store
        .add_run(&created.id, ActorRef::system(), failed)
        .await
        .unwrap();

    assert!(matches!(
        store
            .accept_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap_err(),
        StoreError::InvalidTransition(_)
    ));
}

async fn assert_failed_ticket_with_evidence_cannot_be_closed(store: &impl TicketStore) {
    let created = store
        .create_ticket(
            "Failed close review".to_string(),
            "Do not close evidence from a failed execution.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let mut failed = successful_run(&created.id);
    failed.status = RunStatus::Failed;
    store
        .add_run(&created.id, ActorRef::system(), failed)
        .await
        .unwrap();

    let error = store
        .close_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap_err();

    assert!(matches!(error, StoreError::InvalidTransition(_)));
    assert_eq!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::Failed
    );
    assert!(store
        .ticket_events(&created.id)
        .await
        .unwrap()
        .iter()
        .all(|event| event.kind != TicketEventKind::TicketClosed));
}

#[tokio::test]
async fn memory_failed_ticket_with_evidence_cannot_be_closed() {
    assert_failed_ticket_with_evidence_cannot_be_closed(&InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_failed_ticket_with_evidence_cannot_be_closed() {
    let path = temp_store_path("tea-store-sqlite-failed-close");
    let store = SqliteTicketStore::open(&path).unwrap();

    assert_failed_ticket_with_evidence_cannot_be_closed(&store).await;

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn add_run_rejects_ticket_mismatch() {
    let store = InMemoryTicketStore::default();
    let owner = store
        .create_ticket(
            "Owner".to_string(),
            "Run declares this ticket.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let other = store
        .create_ticket(
            "Other".to_string(),
            "This ticket must not receive another ticket's run.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let run = successful_run(&owner.id);

    std::thread::sleep(std::time::Duration::from_millis(5));
    let err = store
        .add_run(&other.id, ActorRef::system(), run)
        .await
        .unwrap_err();

    assert!(matches!(err, StoreError::InvalidRunTransition(_)));
    assert!(store.list_runs(&owner.id).await.unwrap().is_empty());
    assert!(store.list_runs(&other.id).await.unwrap().is_empty());
    assert_eq!(
        store.get_ticket(&other.id).await.unwrap().updated_at,
        other.updated_at
    );
}

#[tokio::test]
async fn sqlite_add_run_rejects_ticket_mismatch() {
    let path = temp_store_path("tea-store-sqlite-add-run-ticket-mismatch");
    let store = SqliteTicketStore::open(&path).unwrap();
    let owner = store
        .create_ticket(
            "Owner".to_string(),
            "Run declares this ticket.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let other = store
        .create_ticket(
            "Other".to_string(),
            "This ticket must not receive another ticket's run.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let run = successful_run(&owner.id);

    std::thread::sleep(std::time::Duration::from_millis(5));
    let err = store
        .add_run(&other.id, ActorRef::system(), run)
        .await
        .unwrap_err();

    assert!(matches!(err, StoreError::InvalidRunTransition(_)));
    assert!(store.list_runs(&owner.id).await.unwrap().is_empty());
    assert!(store.list_runs(&other.id).await.unwrap().is_empty());
    assert_eq!(
        store.get_ticket(&other.id).await.unwrap().updated_at,
        other.updated_at
    );

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn blocked_ticket_rejects_runs() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Rejected".to_string(),
            "A rejected ticket must not run automatically.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .set_approval_policy(
            &created.id,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    let blocked = store
        .reject_approval(
            &created.id,
            ActorRef::human("vmjcv"),
            "Not acceptable".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status, TicketStatus::Blocked);

    let err = store
        .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
        .await
        .unwrap_err();

    assert!(matches!(err, StoreError::InvalidTransition(_)));
    assert_eq!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::Blocked
    );
    assert!(store.list_runs(&created.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn sqlite_blocked_ticket_rejects_runs() {
    let path = temp_store_path("tea-store-sqlite-blocked-run");
    let store = SqliteTicketStore::open(&path).unwrap();
    let created = store
        .create_ticket(
            "Rejected".to_string(),
            "A rejected ticket must not run automatically.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .set_approval_policy(
            &created.id,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    let blocked = store
        .reject_approval(
            &created.id,
            ActorRef::human("vmjcv"),
            "Not acceptable".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(blocked.status, TicketStatus::Blocked);

    let err = store
        .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
        .await
        .unwrap_err();

    assert!(matches!(err, StoreError::InvalidTransition(_)));
    assert_eq!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::Blocked
    );
    assert!(store.list_runs(&created.id).await.unwrap().is_empty());

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn close_requires_evidence() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Smoke".to_string(),
            "Create a safe plan.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();

    let err = store
        .close_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::EvidenceRequired));
}

async fn assert_close_requires_completion_approval(store: &impl TicketStore) {
    let created = store
        .create_ticket_with_policy(
            "Completion approval".to_string(),
            "Require final human approval inside the store transaction.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::HumanBeforeCompletion,
        )
        .await
        .unwrap();
    store
        .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
        .await
        .unwrap();

    let error = store
        .close_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::ApprovalRequired));
    assert_ne!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::Closed
    );

    store
        .grant_approval(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();
    assert_eq!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::Completed
    );
    let closed = store
        .close_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();
    assert_eq!(closed.status, TicketStatus::Closed);
}

async fn assert_policy_changes_invalidate_prior_approval(store: &impl TicketStore) {
    let created = store
        .create_ticket_with_policy(
            "Policy-bound approval".to_string(),
            "Approval must not survive a change to its governing policy.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::HumanBeforeExecute,
        )
        .await
        .unwrap();
    store
        .grant_approval(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();
    store
        .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
        .await
        .unwrap();
    assert!(store.has_approval(&created.id).await.unwrap());

    store
        .set_approval_policy(
            &created.id,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::HumanBeforeCompletion,
        )
        .await
        .unwrap();
    assert!(!store.has_approval(&created.id).await.unwrap());
    assert!(matches!(
        store
            .close_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap_err(),
        StoreError::ApprovalRequired
    ));

    store
        .grant_approval(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();
    store
        .set_approval_policy(
            &created.id,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::HumanBeforeCompletion,
        )
        .await
        .unwrap();
    assert!(store.has_approval(&created.id).await.unwrap());
    assert_eq!(
        store
            .close_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap()
            .status,
        TicketStatus::Closed
    );

    let analyzed = store
        .create_ticket_with_policy(
            "Analysis policy change".to_string(),
            "Analysis recommendations must invalidate earlier approval.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::HumanBeforeExecute,
        )
        .await
        .unwrap();
    store
        .grant_approval(&analyzed.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();
    store
        .set_analysis(
            &analyzed.id,
            ActorRef::system(),
            TicketAnalysis {
                intent: "policy review".to_string(),
                target_components: vec!["Tea".to_string()],
                target_paths: vec![],
                constraints: vec![],
                acceptance_criteria: vec!["approval is policy-bound".to_string()],
                missing_context: vec![],
                risk_assessment: RiskLevel::Medium,
                confidence: 0.9,
                recommended_policy: ApprovalPolicy::ManualOnly,
                recommended_workflow: "manual review".to_string(),
            },
        )
        .await
        .unwrap();
    assert!(!store.has_approval(&analyzed.id).await.unwrap());
}

#[tokio::test]
async fn memory_policy_changes_invalidate_prior_approval() {
    assert_policy_changes_invalidate_prior_approval(&InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_policy_changes_invalidate_prior_approval() {
    let path = temp_store_path("tea-store-sqlite-policy-bound-approval");
    let store = SqliteTicketStore::open(&path).unwrap();

    assert_policy_changes_invalidate_prior_approval(&store).await;

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn memory_close_checks_completion_approval_atomically() {
    assert_close_requires_completion_approval(&InMemoryTicketStore::default()).await;
}

#[tokio::test]
async fn sqlite_close_checks_completion_approval_atomically() {
    let path = temp_store_path("tea-store-sqlite-close-approval");
    let store = SqliteTicketStore::open(&path).unwrap();

    assert_close_requires_completion_approval(&store).await;

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn accept_requires_evidence() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Review".to_string(),
            "Do not accept work before evidence exists.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();

    let err = store
        .accept_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::EvidenceRequired));

    let ticket = store.get_ticket(&created.id).await.unwrap();
    assert_ne!(ticket.status, TicketStatus::Accepted);
}

#[tokio::test]
async fn accept_after_run_evidence_appends_accepted_event() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Review".to_string(),
            "Accept only after evidence exists.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
        .await
        .unwrap();

    let accepted = store
        .accept_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();

    assert_eq!(accepted.status, TicketStatus::Accepted);
    let events = store.ticket_events(&created.id).await.unwrap();
    assert_eq!(events.last().unwrap().kind, TicketEventKind::HumanAccepted);
}

#[tokio::test]
async fn close_after_run_evidence_appends_closed_event() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Smoke".to_string(),
            "Create a safe plan.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .add_run(
            &created.id,
            ActorRef::system(),
            Run {
                id: RunId::new(),
                ticket_id: created.id.clone(),
                loom_session_id: Some("mock".to_string()),
                status: RunStatus::Succeeded,
                evidence: Some(RunEvidence {
                    summary: "done".to_string(),
                    commands: vec![],
                    artifacts: vec![],
                    risks: vec![],
                }),
            },
        )
        .await
        .unwrap();
    store
        .close_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();

    let events = store.ticket_events(&created.id).await.unwrap();
    assert_eq!(events.last().unwrap().kind, TicketEventKind::TicketClosed);
}

#[tokio::test]
async fn cancel_ticket_appends_cancelled_event_and_freezes_ticket() {
    let store = InMemoryTicketStore::default();
    let created = store
        .create_ticket(
            "Cancel".to_string(),
            "Stop this work order before execution.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();

    let cancelled = store
        .cancel_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap();

    assert_eq!(cancelled.status, TicketStatus::Cancelled);
    let events = store.ticket_events(&created.id).await.unwrap();
    assert_eq!(
        events.last().unwrap().kind,
        TicketEventKind::TicketCancelled
    );

    let comment_error = store
        .add_comment(
            &created.id,
            ActorRef::human("vmjcv"),
            "Do not mutate cancelled tickets.".to_string(),
        )
        .await
        .unwrap_err();
    assert!(matches!(comment_error, StoreError::InvalidTransition(_)));
}

#[tokio::test]
async fn sqlite_cancel_ticket_persists_cancelled_state_after_reopen() {
    let path = temp_store_path("tea-store-sqlite-cancelled-state");
    let created = {
        let store = SqliteTicketStore::open(&path).unwrap();
        let created = store
            .create_ticket(
                "Cancel persistent".to_string(),
                "Cancelled tickets stay terminal after daemon restart.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        let cancelled = store
            .cancel_ticket(&created.id, ActorRef::human("vmjcv"))
            .await
            .unwrap();
        assert_eq!(cancelled.status, TicketStatus::Cancelled);
        created
    };

    let reopened = SqliteTicketStore::open(&path).unwrap();
    let ticket = reopened.get_ticket(&created.id).await.unwrap();
    assert_eq!(ticket.status, TicketStatus::Cancelled);
    let events = reopened.ticket_events(&created.id).await.unwrap();
    assert_eq!(
        events.last().unwrap().kind,
        TicketEventKind::TicketCancelled
    );
    let error = reopened
        .add_comment(
            &created.id,
            ActorRef::human("vmjcv"),
            "Persistent cancelled tickets stay immutable.".to_string(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidTransition(_)));

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn closed_ticket_rejects_in_memory_mutations() {
    let store = InMemoryTicketStore::default();
    let closed = create_closed_ticket(&store).await;

    let comment_error = store
        .add_comment(
            &closed.id,
            ActorRef::human("vmjcv"),
            "Do not mutate closed tickets.".to_string(),
        )
        .await
        .unwrap_err();
    assert!(matches!(comment_error, StoreError::InvalidTransition(_)));

    let approval_error = store
        .grant_approval(&closed.id, ActorRef::human("vmjcv"))
        .await
        .unwrap_err();
    assert!(matches!(approval_error, StoreError::InvalidTransition(_)));

    let run_error = store
        .add_run(&closed.id, ActorRef::system(), successful_run(&closed.id))
        .await
        .unwrap_err();
    assert!(matches!(run_error, StoreError::InvalidTransition(_)));
}

#[tokio::test]
async fn closed_ticket_rejects_sqlite_mutations_after_reopen() {
    let path = temp_store_path("tea-store-closed-state");
    let closed = {
        let store = SqliteTicketStore::open(&path).unwrap();
        create_closed_ticket(&store).await
    };

    let reopened = SqliteTicketStore::open(&path).unwrap();
    let error = reopened
        .add_comment(
            &closed.id,
            ActorRef::human("vmjcv"),
            "Persistent closed tickets stay immutable.".to_string(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::InvalidTransition(_)));

    let ticket = reopened.get_ticket(&closed.id).await.unwrap();
    assert_eq!(ticket.status, TicketStatus::Closed);

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn sqlite_store_preserves_ticket_and_events_after_reopen() {
    let path = temp_store_path("tea-store");

    let created = {
        let store = SqliteTicketStore::open(&path).unwrap();
        let created = store
            .create_ticket(
                "Persistent".to_string(),
                "Survive daemon restart.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .add_comment(
                &created.id,
                ActorRef::human("vmjcv"),
                "Persist this comment event.".to_string(),
            )
            .await
            .unwrap();
        created
    };

    let reopened = SqliteTicketStore::open(&path).unwrap();
    let ticket = reopened.get_ticket(&created.id).await.unwrap();
    assert_eq!(ticket.title, "Persistent");
    let events = reopened.ticket_events(&created.id).await.unwrap();
    assert_eq!(
        events.iter().map(|event| &event.kind).collect::<Vec<_>>(),
        vec![
            &TicketEventKind::TicketCreated,
            &TicketEventKind::CommentAdded
        ]
    );
    let comments = reopened.ticket_comments(&created.id).await.unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].body, "Persist this comment event.");

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn sqlite_store_preserves_policy_update_after_reopen() {
    let path = temp_store_path("tea-store-policy");

    let created = {
        let store = SqliteTicketStore::open(&path).unwrap();
        let created = store
            .create_ticket(
                "Policy".to_string(),
                "Persist explicit approval policy.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
        store
            .set_approval_policy(
                &created.id,
                ActorRef::human("vmjcv"),
                ApprovalPolicy::ManualOnly,
            )
            .await
            .unwrap();
        created
    };

    let reopened = SqliteTicketStore::open(&path).unwrap();
    let ticket = reopened.get_ticket(&created.id).await.unwrap();
    assert_eq!(ticket.approval_policy, ApprovalPolicy::ManualOnly);
    let events = reopened.ticket_events(&created.id).await.unwrap();
    assert_eq!(events.last().unwrap().kind, TicketEventKind::PolicyUpdated);

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn sqlite_store_initializes_schema_migration_metadata() {
    let path = temp_store_path("tea-store-schema-meta");

    {
        let store = SqliteTicketStore::open(&path).unwrap();
        store
            .create_ticket(
                "Metadata".to_string(),
                "Schema version metadata should be durable.".to_string(),
                TicketSource::Human,
                ActorRef::human("vmjcv"),
            )
            .await
            .unwrap();
    }

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row(
            "SELECT version FROM schema_migrations ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, CURRENT_SQLITE_SCHEMA_VERSION);

    let _ = std::fs::remove_file(path);
}

#[test]
fn sqlite_open_removes_redundant_ordinal_indexes() {
    let path = temp_store_path("tea-store-redundant-indexes");
    {
        let _store = SqliteTicketStore::open(&path).unwrap();
    }
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
                CREATE INDEX idx_tickets_ordinal ON tickets(ordinal);
                CREATE INDEX idx_comments_ticket_ordinal ON comments(ticket_id, ordinal);
                CREATE INDEX idx_events_ticket_ordinal ON events(ticket_id, ordinal);
                CREATE INDEX idx_runs_ticket_ordinal ON runs(ticket_id, ordinal);
                "#,
        )
        .unwrap();
    }

    {
        let _store = SqliteTicketStore::open(&path).unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    let mut statement = conn
        .prepare(
            "SELECT name FROM sqlite_master \
                 WHERE type = 'index' AND name LIKE '%ordinal' ORDER BY name",
        )
        .unwrap();
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();

    assert_eq!(
        names,
        vec![
            "idx_tickets_source_ordinal",
            "idx_tickets_status_ordinal",
            "uidx_comments_ticket_ordinal",
            "uidx_events_ticket_ordinal",
            "uidx_runs_ticket_ordinal",
            "uidx_tickets_ordinal",
        ]
    );

    drop(statement);
    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn sqlite_store_reopen_does_not_duplicate_schema_migration_records() {
    let path = temp_store_path("tea-store-schema-reopen");

    {
        let _store = SqliteTicketStore::open(&path).unwrap();
    }
    {
        let _store = SqliteTicketStore::open(&path).unwrap();
    }

    let conn = Connection::open(&path).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, CURRENT_SQLITE_SCHEMA_VERSION);

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn sqlite_store_upgrades_legacy_schema_without_metadata() {
    let path = temp_store_path("tea-store-legacy-schema");
    {
        let conn = Connection::open(&path).unwrap();
        create_sqlite_v1_schema(&conn).unwrap();
        assert!(!sqlite_table_exists(&conn, "schema_migrations").unwrap());
    }

    let _store = SqliteTicketStore::open(&path).unwrap();

    let conn = Connection::open(&path).unwrap();
    let version: i64 = conn
        .query_row(
            "SELECT version FROM schema_migrations ORDER BY version DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, CURRENT_SQLITE_SCHEMA_VERSION);

    let _ = std::fs::remove_file(path);
}

#[test]
fn sqlite_store_upgrades_v2_schema_with_idempotency_table() {
    let path = temp_store_path("tea-store-v2-idempotency-migration");
    {
        let conn = Connection::open(&path).unwrap();
        ensure_schema_migrations_table(&conn).unwrap();
        create_sqlite_v1_schema(&conn).unwrap();
        record_sqlite_schema_version(&conn, 1).unwrap();
        create_sqlite_v2_schema(&conn).unwrap();
        record_sqlite_schema_version(&conn, 2).unwrap();
        assert!(!sqlite_table_exists(&conn, "idempotency_keys").unwrap());
    }

    let _store = SqliteTicketStore::open(&path).unwrap();

    let conn = Connection::open(&path).unwrap();
    assert!(sqlite_table_exists(&conn, "idempotency_keys").unwrap());
    assert_eq!(
        applied_sqlite_schema_version(&conn).unwrap(),
        CURRENT_SQLITE_SCHEMA_VERSION
    );
    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn sqlite_store_upgrades_v3_schema_with_ticket_filter_columns() {
    let path = temp_store_path("tea-store-v3-ticket-filter-migration");
    {
        let conn = Connection::open(&path).unwrap();
        ensure_schema_migrations_table(&conn).unwrap();
        create_sqlite_v1_schema(&conn).unwrap();
        record_sqlite_schema_version(&conn, 1).unwrap();
        create_sqlite_v2_schema(&conn).unwrap();
        record_sqlite_schema_version(&conn, 2).unwrap();
        create_sqlite_v3_schema(&conn).unwrap();
        record_sqlite_schema_version(&conn, 3).unwrap();
        conn.execute(
            "INSERT INTO tickets (id, ordinal, json) VALUES \
                 ('ticket-v3', 0, '{\"status\":\"open\",\"source\":\"human\"}')",
            [],
        )
        .unwrap();
        assert!(!sqlite_index_exists(&conn, "idx_tickets_status_ordinal").unwrap());
        assert!(!sqlite_index_exists(&conn, "idx_tickets_source_ordinal").unwrap());
    }

    let _store = SqliteTicketStore::open(&path).unwrap();

    let conn = Connection::open(&path).unwrap();
    assert!(sqlite_index_exists(&conn, "idx_tickets_status_ordinal").unwrap());
    assert!(sqlite_index_exists(&conn, "idx_tickets_source_ordinal").unwrap());
    // Pre-migration rows expose the generated filter columns.
    let (status, source): (String, String) = conn
        .query_row(
            "SELECT status, source FROM tickets WHERE id = 'ticket-v3'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((status.as_str(), source.as_str()), ("open", "human"));
    assert_eq!(
        applied_sqlite_schema_version(&conn).unwrap(),
        CURRENT_SQLITE_SCHEMA_VERSION
    );
    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn sqlite_concurrent_initialization_and_writes_keep_ordinals_unique() {
    let path = temp_store_path("tea-store-concurrent-writers");
    let workers = 4;
    let writes_per_worker = 20;
    let barrier = Arc::new(std::sync::Barrier::new(workers));
    let handles = (0..workers)
        .map(|worker| {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || -> Result<(), StoreError> {
                let mut conn = Connection::open(path)?;
                barrier.wait();
                init_sqlite(&mut conn)?;
                for index in 0..writes_per_worker {
                    let actor = ActorRef::human(format!("worker-{worker}"));
                    let ticket = Ticket::new_with_options(
                        TicketId::new(),
                        format!("Concurrent {worker}-{index}"),
                        "Concurrent SQLite writer regression test".to_string(),
                        TicketSource::Human,
                        actor.clone(),
                        Ticket::default_approval_policy_for_source(TicketSource::Human),
                        TicketCreateOptions::default(),
                    );
                    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    insert_ticket(&tx, &ticket)?;
                    push_sqlite_event(&tx, &ticket.id, actor, TicketEventKind::TicketCreated)?;
                    tx.commit()?;
                }
                Ok(())
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().expect("writer thread panicked").unwrap();
    }

    let conn = Connection::open(&path).unwrap();
    let (ticket_count, distinct_ticket_ordinals): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT ordinal) FROM tickets",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let (event_count, distinct_event_ordinals): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT ticket_id || ':' || ordinal) FROM events",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let expected = i64::try_from(workers * writes_per_worker).unwrap();
    assert_eq!(
        (ticket_count, distinct_ticket_ordinals),
        (expected, expected)
    );
    assert_eq!((event_count, distinct_event_ordinals), (expected, expected));
    assert_eq!(
        applied_sqlite_schema_version(&conn).unwrap(),
        CURRENT_SQLITE_SCHEMA_VERSION
    );

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[test]
fn sqlite_v2_rejects_duplicate_ordinals_at_the_database_boundary() {
    let path = temp_store_path("tea-store-unique-ordinals");
    let mut conn = Connection::open(&path).unwrap();
    init_sqlite(&mut conn).unwrap();

    conn.execute(
        "INSERT INTO tickets (id, ordinal, json) VALUES ('ticket-a', 0, '{}')",
        [],
    )
    .unwrap();
    let duplicate = conn.execute(
        "INSERT INTO tickets (id, ordinal, json) VALUES ('ticket-b', 0, '{}')",
        [],
    );
    assert!(duplicate.is_err());

    drop(conn);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn sqlite_store_rejects_future_schema_version() {
    let path = temp_store_path("tea-store-future-schema");
    {
        let conn = Connection::open(&path).unwrap();
        create_sqlite_v1_schema(&conn).unwrap();
        ensure_schema_migrations_table(&conn).unwrap();
        record_sqlite_schema_version(&conn, CURRENT_SQLITE_SCHEMA_VERSION + 1).unwrap();
    }

    let error = match SqliteTicketStore::open(&path) {
        Ok(_) => panic!("future schema version should be rejected"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        StoreError::UnsupportedSchemaVersion { found, supported }
            if found == CURRENT_SQLITE_SCHEMA_VERSION + 1
                && supported == CURRENT_SQLITE_SCHEMA_VERSION
    ));

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn sqlite_store_rolls_back_schema_changes_when_migration_record_fails() {
    let path = temp_store_path("tea-store-migration-rollback");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at TEXT NOT NULL
                );
                "#,
        )
        .unwrap();
    }

    let error = match SqliteTicketStore::open(&path) {
        Ok(_) => panic!("migration should fail when schema_migrations cannot record a version"),
        Err(error) => error,
    };
    match error {
        StoreError::Database(error) => {
            assert!(error.to_string().contains("NOT NULL constraint failed"));
        }
        other => panic!("expected sqlite constraint error, got {other:?}"),
    }

    let conn = Connection::open(&path).unwrap();
    assert!(!sqlite_table_exists(&conn, "tickets").unwrap());
    assert!(!sqlite_table_exists(&conn, "comments").unwrap());
    assert!(!sqlite_table_exists(&conn, "events").unwrap());

    let _ = std::fs::remove_file(path);
}

async fn create_closed_ticket<S>(store: &S) -> Ticket
where
    S: TicketStore,
{
    let created = store
        .create_ticket(
            "Closed".to_string(),
            "This work order has finished.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .add_run(&created.id, ActorRef::system(), successful_run(&created.id))
        .await
        .unwrap();
    store
        .close_ticket(&created.id, ActorRef::human("vmjcv"))
        .await
        .unwrap()
}

async fn assert_historical_run_update_preserves_latest_ticket_status<S>(store: &S)
where
    S: TicketStore,
{
    let created = store
        .create_ticket_with_policy(
            "Historical run update".to_string(),
            "Keep the latest run authoritative for the ticket status.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    let mut historical = running_run(&created.id);
    historical.status = RunStatus::Stopped;
    store
        .add_run(&created.id, ActorRef::loom("test-loom"), historical.clone())
        .await
        .unwrap();
    let latest = successful_run(&created.id);
    store
        .add_run(&created.id, ActorRef::loom("test-loom"), latest)
        .await
        .unwrap();

    historical.status = RunStatus::Retrying;
    store
        .update_run(&created.id, ActorRef::loom("test-loom"), historical.clone())
        .await
        .unwrap();

    assert_eq!(
        store.get_run(&historical.id).await.unwrap().status,
        RunStatus::Retrying
    );
    assert_eq!(
        store.get_ticket(&created.id).await.unwrap().status,
        TicketStatus::Completed
    );
}

fn successful_run(ticket_id: &TicketId) -> Run {
    Run {
        id: RunId::new(),
        ticket_id: ticket_id.clone(),
        loom_session_id: Some("mock".to_string()),
        status: RunStatus::Succeeded,
        evidence: Some(RunEvidence {
            summary: "done".to_string(),
            commands: vec![],
            artifacts: vec![],
            risks: vec![],
        }),
    }
}

fn running_run(ticket_id: &TicketId) -> Run {
    Run {
        id: RunId::new(),
        ticket_id: ticket_id.clone(),
        loom_session_id: Some("running-session".to_string()),
        status: RunStatus::Running,
        evidence: None,
    }
}

fn temp_store_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "{}-{}-{}.sqlite",
        prefix,
        std::process::id(),
        chrono::Utc::now()
            .timestamp_nanos_opt()
            .expect("current timestamp should fit in nanos")
    ))
}

fn sqlite_table_exists(conn: &Connection, table_name: &str) -> Result<bool, StoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![table_name],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(exists == 1)
}

fn sqlite_index_exists(conn: &Connection, index_name: &str) -> Result<bool, StoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1)",
        params![index_name],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(exists == 1)
}
