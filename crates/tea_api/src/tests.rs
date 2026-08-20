use super::*;
use axum::body::Body;
use axum::http::Request;
use tea_store::SqliteTicketStore;
use tower::Service;
use tower::ServiceExt;

#[test]
fn internal_and_upstream_errors_have_stable_public_messages() {
    let store_error = ApiError::from(StoreError::Io(std::io::Error::other(
        "private database path C:\\private\\tea.sqlite",
    )));
    assert_eq!(store_error.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(store_error.message, "internal server error");
    assert!(!store_error.message.contains("private"));

    let brain_error = ApiError::from(BrainError::ProviderUnavailable(
        "private Brain endpoint detail".to_string(),
    ));
    assert_eq!(brain_error.status, StatusCode::BAD_GATEWAY);
    assert_eq!(brain_error.message, "BrainProvider unavailable");
    assert!(!brain_error.message.contains("endpoint detail"));

    let loom_error = ApiError::from(tea_loom::LoomError::InvalidResponse(
        "private Loom response detail".to_string(),
    ));
    assert_eq!(loom_error.status, StatusCode::BAD_GATEWAY);
    assert_eq!(loom_error.message, "Loom service unavailable");
    assert!(!loom_error.message.contains("response detail"));
}

#[tokio::test]
async fn health_returns_ok() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn json_request_body_limit_rejects_oversized_payloads() {
    let app = test_router();
    let body = format!(
        "{{\"title\":\"large\",\"description\":\"{}\"}}",
        "x".repeat(MAX_HTTP_JSON_REQUEST_BYTES)
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn create_ticket_rejects_oversized_semantic_fields() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "x".repeat(MAX_TICKET_TITLE_BYTES + 1),
                        "description": "bounded"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["error"],
        format!("ticket title must be at most {MAX_TICKET_TITLE_BYTES} UTF-8 bytes")
    );

    let labels = (0..=MAX_TICKET_LABELS)
        .map(|index| format!("label-{index}"))
        .collect::<Vec<_>>();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "bounded",
                        "description": "bounded",
                        "labels": labels
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ticket_mutations_reject_oversized_persisted_text() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"bounded","description":"bounded"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: Ticket = serde_json::from_slice(&bytes).unwrap();

    let cases = [
        (
            "PATCH",
            format!("/v1/tickets/{}", ticket.id),
            json!({"priority":"x".repeat(MAX_TICKET_PRIORITY_BYTES + 1)}),
        ),
        (
            "POST",
            format!("/v1/tickets/{}/comments", ticket.id),
            json!({"body":"x".repeat(MAX_COMMENT_BODY_BYTES + 1)}),
        ),
        (
            "POST",
            format!("/v1/tickets/{}/reject", ticket.id),
            json!({"reason":"x".repeat(MAX_REJECTION_REASON_BYTES + 1)}),
        ),
    ];

    for (method, uri, body) in cases {
        let response = app
            .call(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn hook_intake_rejects_excessive_attachment_metadata() {
    let mut app = test_router();
    let attachments = (0..=MAX_HOOK_ATTACHMENTS)
        .map(|index| json!({"kind":"reference","reference":format!("item-{index}")}))
        .collect::<Vec<_>>();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/intake/hook")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "source":"hook",
                        "text":"bounded",
                        "context":{
                            "active_window":null,
                            "selection_text":null,
                            "ocr_text":null,
                            "screenshot_ref":null,
                            "cwd":null,
                            "app":null
                        },
                        "attachments":attachments
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/intake/hook")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "source":"hook",
                        "text":"x".repeat(MAX_HOOK_TEXT_BYTES),
                        "context":{
                            "active_window":null,
                            "selection_text":"y".repeat(MAX_HOOK_CONTEXT_FIELD_BYTES),
                            "ocr_text":"z".repeat(MAX_HOOK_CONTEXT_FIELD_BYTES),
                            "screenshot_ref":null,
                            "cwd":null,
                            "app":null
                        },
                        "attachments":[]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn v1_ticket_metrics_aggregates_in_one_response() {
    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Metrics".to_string(),
            "body".to_string(),
            tea_core::TicketSource::Human,
            tea_core::ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .add_comment(
            &ticket.id,
            tea_core::ActorRef::human("vmjcv"),
            "note".to_string(),
        )
        .await
        .unwrap();

    let app = router(AppState::new(
        store.clone(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    ));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/tickets/metrics")
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let entries = body.as_array().expect("metrics is an array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["ticket_id"], json!(ticket.id));
    assert_eq!(entries[0]["comments_count"], 1);
    assert_eq!(entries[0]["runs_count"], 0);
    assert_eq!(entries[0]["latest_comment"]["body"], "note");
}

#[tokio::test]
async fn v1_ticket_list_and_metrics_support_cursor_pages_and_filters() {
    let store = InMemoryTicketStore::default();
    let mut tickets = Vec::new();
    for (title, source) in [
        ("A", TicketSource::Human),
        ("B", TicketSource::Hook),
        ("C", TicketSource::Human),
        ("D", TicketSource::Hook),
        ("E", TicketSource::Human),
    ] {
        tickets.push(
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
            &tickets[2].id,
            ActorRef::human("vmjcv"),
            "paged comment".to_string(),
        )
        .await
        .unwrap();
    let mut app = router(AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    ));

    let response = app
        .call(
            Request::builder()
                .uri("/v1/tickets?limit=2")
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let first: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(first["items"][0]["title"], "A");
    assert_eq!(first["items"][1]["title"], "B");
    assert_eq!(first["next_cursor"], encode_ticket_cursor(1, None, None));

    let cursor = first["next_cursor"].as_str().unwrap();
    let response = app
        .call(
            Request::builder()
                .uri(format!("/v1/tickets?limit=2&cursor={cursor}"))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let second: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(second["items"][0]["title"], "C");
    assert_eq!(second["items"][1]["title"], "D");
    assert_eq!(second["next_cursor"], encode_ticket_cursor(3, None, None));

    let response = app
        .call(
            Request::builder()
                .uri("/v1/tickets?source=hook&limit=1")
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let filtered: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(filtered["items"][0]["title"], "B");
    assert_eq!(
        filtered["next_cursor"],
        encode_ticket_cursor(1, None, Some(TicketSource::Hook))
    );

    let response = app
        .call(
            Request::builder()
                .uri(format!(
                    "/v1/tickets/metrics?limit=2&cursor={}",
                    encode_ticket_cursor(1, None, None)
                ))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let metrics: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(metrics["items"][0]["ticket_id"], json!(tickets[2].id));
    assert_eq!(metrics["items"][0]["comments_count"], 1);
    assert_eq!(metrics["items"][1]["ticket_id"], json!(tickets[3].id));
    assert_eq!(metrics["next_cursor"], encode_ticket_cursor(3, None, None));
}

#[tokio::test]
async fn v1_ticket_pagination_rejects_invalid_queries() {
    for uri in [
        "/v1/tickets?limit=0",
        "/v1/tickets?limit=201",
        "/v1/tickets?limit=not-a-number",
        "/v1/tickets?cursor=not-a-cursor",
        "/v1/tickets?status=not-a-status",
        "/v1/tickets?source=not-a-source",
    ] {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "uri={uri}");
    }

    let filter_cursor = encode_ticket_cursor(1, None, Some(TicketSource::Hook));
    let response = test_router()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/tickets?limit=10&cursor={filter_cursor}"))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn v1_ticket_bundle_returns_detail_in_one_response() {
    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Bundle".to_string(),
            "body".to_string(),
            tea_core::TicketSource::Human,
            tea_core::ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .add_comment(
            &ticket.id,
            tea_core::ActorRef::human("vmjcv"),
            "note".to_string(),
        )
        .await
        .unwrap();

    let app = router(AppState::new(
        store.clone(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    ));
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/tickets/{}/bundle", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["ticket"]["id"], json!(ticket.id));
    assert_eq!(
        body["comments"].as_array().expect("comments array").len(),
        1
    );
    assert_eq!(body["comments"][0]["body"], "note");
    assert!(!body["events"].as_array().expect("events array").is_empty());
    assert!(body["analysis"].is_null());
    assert!(body["plan"].is_null());
}

#[tokio::test]
async fn status_reports_memory_store_metadata() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/status")
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["service"], "tea");
    assert_eq!(body["status"], "ok");
    assert_eq!(body["execution_provider"], "mock");
    assert_eq!(
        body["store"],
        json!({
            "backend": "memory",
            "schema_version": null,
            "supported_schema_version": null,
            "idempotency_key_count": null,
            "sqlite_page_count": null,
            "sqlite_freelist_count": null
        })
    );
}

#[tokio::test]
async fn v1_status_requires_auth() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn v1_read_endpoints_require_auth() {
    let mut app = test_router();
    let ticket_response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Auth read smoke",
                        "description": "Verify read endpoints require bearer auth."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ticket_response.status(), StatusCode::OK);
    let ticket: tea_core::Ticket =
        serde_json::from_slice(&body_bytes(ticket_response).await).unwrap();

    for uri in [
        "/v1/configuration".to_string(),
        "/v1/tickets".to_string(),
        format!("/v1/tickets/{}", ticket.id),
        format!("/v1/tickets/{}/comments", ticket.id),
        format!("/v1/tickets/{}/events", ticket.id),
        format!("/v1/tickets/{}/runs", ticket.id),
        format!("/v1/tickets/{}/export/json", ticket.id),
        format!("/v1/tickets/{}/export/markdown", ticket.id),
    ] {
        let response = app
            .call(Request::builder().uri(&uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "uri={uri}");
    }
}

#[tokio::test]
async fn authenticated_v1_read_endpoints_still_work() {
    let mut app = test_router();
    let ticket_response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Authenticated read smoke",
                        "description": "Verify read endpoints still work with bearer auth."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ticket_response.status(), StatusCode::OK);
    let ticket: tea_core::Ticket =
        serde_json::from_slice(&body_bytes(ticket_response).await).unwrap();

    for uri in [
        "/v1/status".to_string(),
        "/v1/configuration".to_string(),
        "/v1/tickets".to_string(),
        format!("/v1/tickets/{}", ticket.id),
        format!("/v1/tickets/{}/comments", ticket.id),
        format!("/v1/tickets/{}/events", ticket.id),
        format!("/v1/tickets/{}/runs", ticket.id),
        format!("/v1/tickets/{}/export/json", ticket.id),
        format!("/v1/tickets/{}/export/markdown", ticket.id),
    ] {
        let response = app
            .call(
                Request::builder()
                    .uri(&uri)
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "uri={uri}");
    }
}

#[tokio::test]
async fn status_reports_sqlite_schema_metadata() {
    let path = temp_store_path("tea-api-status-sqlite");
    let store = SqliteTicketStore::open(&path).unwrap();
    let app = router(AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    ));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/status")
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let store = &body["store"];
    assert_eq!(store["backend"], "sqlite");
    assert_eq!(store["schema_version"], 4);
    assert_eq!(store["supported_schema_version"], 4);
    assert_eq!(store["idempotency_key_count"], 0);
    assert!(store["sqlite_page_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(store["sqlite_freelist_count"].as_u64().is_some());

    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn status_reports_configuration_source() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/status")
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["configuration_source"], "local");
    assert_eq!(body["configuration"]["owner"], "tea");
}

#[tokio::test]
async fn configuration_put_updates_local_config() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("PUT")
                .uri("/v1/configuration")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "notifications_enabled": false,
                        "human_ticket_default_approval_policy": "human_before_completion",
                        "hook_ticket_default_approval_policy": "plan_only"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["configuration_source"], "local");
    assert_eq!(body["config"]["notifications_enabled"], false);
    assert_eq!(
        body["config"]["human_ticket_default_approval_policy"],
        "human_before_completion"
    );
}

#[tokio::test]
async fn configuration_patch_updates_only_requested_fields() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("PATCH")
                .uri("/v1/configuration")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "notifications_enabled": false }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    assert_eq!(body["config"]["notifications_enabled"], false);
    assert_eq!(
        body["config"]["human_ticket_default_approval_policy"],
        "human_before_execute"
    );
    assert_eq!(
        body["config"]["hook_ticket_default_approval_policy"],
        "plan_only"
    );
}

#[tokio::test]
async fn configuration_patch_rejects_empty_and_invalid_updates() {
    for (payload, expected_error) in [
        (
            json!({}),
            "configuration patch must update at least one field",
        ),
        (
            json!({ "human_ticket_default_approval_policy": "not_a_policy" }),
            "invalid approval policy in Tea configuration: not_a_policy",
        ),
    ] {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri("/v1/configuration")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
        assert_eq!(body["error"], expected_error);
    }
}

#[tokio::test]
async fn configuration_patch_rejects_loom_managed_config() {
    let state = AppState::new_with_configuration(
        InMemoryTicketStore::default(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
        ConfigurationRuntime::loom_managed_for_tests("loom://settings/tea"),
    );
    let response = router(state)
        .oneshot(
            Request::builder()
                .method("PATCH")
                .uri("/v1/configuration")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "notifications_enabled": false }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = serde_json::from_slice(&body_bytes(response).await).unwrap();
    assert_eq!(body["error"], "configuration_managed_by_loom");
}

#[test]
fn configuration_runtimes_merge_concurrent_file_backed_patches() {
    let path = temp_config_path("tea-api-config-patch");
    write_local_config_atomic(&path, &TeaConfiguration::default()).unwrap();
    let ownership = ConfigurationOwnership {
        source: ConfigurationSource::Local,
        configuration: ConfigurationDetails {
            owner: ConfigurationOwner::Tea,
            local_config_path: Some(path.display().to_string()),
            loom_base_url: None,
            loom_panel_url: None,
            reason: None,
        },
    };
    let notifications_runtime = ConfigurationRuntime::new_with_local_path(
        ownership.clone(),
        TeaConfiguration::default(),
        Some(path.clone()),
    );
    let policy_runtime = ConfigurationRuntime::new_with_local_path(
        ownership,
        TeaConfiguration::default(),
        Some(path.clone()),
    );
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let notifications = {
        let runtime = notifications_runtime.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            runtime
                .patch_local_config(ConfigurationPatchRequest {
                    notifications_enabled: Some(false),
                    ..ConfigurationPatchRequest::default()
                })
                .unwrap();
        })
    };
    let policy = {
        let runtime = policy_runtime.clone();
        let barrier = barrier.clone();
        std::thread::spawn(move || {
            barrier.wait();
            runtime
                .patch_local_config(ConfigurationPatchRequest {
                    human_ticket_default_approval_policy: Some("manual_only".to_string()),
                    ..ConfigurationPatchRequest::default()
                })
                .unwrap();
        })
    };
    barrier.wait();
    notifications.join().unwrap();
    policy.join().unwrap();

    let refreshed = notifications_runtime.response().unwrap().config;
    assert!(!refreshed.notifications_enabled);
    assert_eq!(
        refreshed.human_ticket_default_approval_policy,
        "manual_only"
    );
    assert_eq!(refreshed.hook_ticket_default_approval_policy, "plan_only");

    let policy_snapshot = policy_runtime.response().unwrap().config;
    assert_eq!(policy_snapshot, refreshed);
    std::fs::remove_file(&path).unwrap();
    let rebuilt = policy_runtime
        .patch_local_config(ConfigurationPatchRequest {
            hook_ticket_default_approval_policy: Some("human_before_execute".to_string()),
            ..ConfigurationPatchRequest::default()
        })
        .unwrap()
        .config;
    assert!(!rebuilt.notifications_enabled);
    assert_eq!(rebuilt.human_ticket_default_approval_policy, "manual_only");
    assert_eq!(
        rebuilt.hook_ticket_default_approval_policy,
        "human_before_execute"
    );
    assert_eq!(
        read_local_config_file(&path).unwrap(),
        Some(rebuilt.clone())
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(config_lock_path_for_test(&path));
}

#[test]
fn configuration_runtime_refreshes_same_size_edit_with_preserved_mtime() {
    let path = temp_config_path("tea-api-config-refresh");
    let initial = TeaConfiguration {
        human_ticket_default_approval_policy: "manual_only".to_string(),
        ..TeaConfiguration::default()
    };
    write_local_config_atomic(&path, &initial).unwrap();
    let initial_metadata = std::fs::metadata(&path).unwrap();
    let initial_modified = initial_metadata.modified().unwrap();

    let ownership = ConfigurationOwnership {
        source: ConfigurationSource::Local,
        configuration: ConfigurationDetails {
            owner: ConfigurationOwner::Tea,
            local_config_path: Some(path.display().to_string()),
            loom_base_url: None,
            loom_panel_url: None,
            reason: None,
        },
    };
    let runtime = ConfigurationRuntime::new_with_local_path(
        ownership,
        TeaConfiguration::default(),
        Some(path.clone()),
    );
    assert_eq!(
        runtime
            .response()
            .unwrap()
            .config
            .human_ticket_default_approval_policy,
        "manual_only"
    );

    let mut edited = initial.clone();
    edited.human_ticket_default_approval_policy = "always_auto".to_string();
    let initial_json = tea_config::encode_local_config(&initial).unwrap();
    let edited_json = tea_config::encode_local_config(&edited).unwrap();
    assert_eq!(initial_json.len(), edited_json.len());
    std::fs::write(&path, edited_json).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(initial_modified))
        .unwrap();
    let edited_metadata = std::fs::metadata(&path).unwrap();
    assert_eq!(edited_metadata.len(), initial_metadata.len());
    assert_eq!(edited_metadata.modified().unwrap(), initial_modified);

    assert_eq!(
        runtime
            .response()
            .unwrap()
            .config
            .human_ticket_default_approval_policy,
        "always_auto"
    );

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(config_lock_path_for_test(&path));
}

#[tokio::test]
async fn configuration_put_rejects_loom_managed_config() {
    let state = AppState::new_with_configuration(
        InMemoryTicketStore::default(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
        ConfigurationRuntime::loom_managed_for_tests("loom://settings/tea"),
    );
    let mut app = router(state);

    let response = app
        .call(
            Request::builder()
                .method("PUT")
                .uri("/v1/configuration")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "notifications_enabled": false,
                        "human_ticket_default_approval_policy": "human_before_completion",
                        "hook_ticket_default_approval_policy": "plan_only"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "configuration_managed_by_loom");
}

#[test]
fn loom_runtime_config_can_replace_startup_snapshot() {
    let runtime = ConfigurationRuntime::loom_managed_for_tests("loom://settings/tea");
    let response = runtime
        .replace_runtime_config_from_loom(TeaConfiguration {
            notifications_enabled: false,
            human_ticket_default_approval_policy: "manual_only".to_string(),
            hook_ticket_default_approval_policy: "plan_only".to_string(),
        })
        .expect("replace Loom runtime config");

    assert_eq!(
        response.configuration_source,
        ConfigurationSource::LoomManaged
    );
    assert!(!response.config.notifications_enabled);
    assert_eq!(
        response.config.human_ticket_default_approval_policy,
        "manual_only"
    );
}

#[tokio::test]
async fn configuration_put_rejects_unknown_approval_policy() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("PUT")
                .uri("/v1/configuration")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "notifications_enabled": true,
                        "human_ticket_default_approval_policy": "not_a_policy",
                        "hook_ticket_default_approval_policy": "plan_only"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        body["error"],
        "invalid approval policy in Tea configuration: not_a_policy"
    );
}

#[tokio::test]
async fn settings_page_exposes_local_configuration_ui() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("Tea Settings"));
    assert!(body.contains("data-configuration-source=\"local\""));
    assert!(body.contains("notifications_enabled"));
    assert!(body.contains("human_ticket_default_approval_policy"));
    assert!(body.contains("hook_ticket_default_approval_policy"));
    assert!(body.contains("Save Tea local settings"));
    assert!(body.contains("method: 'PATCH'"));
}

#[tokio::test]
async fn settings_page_links_to_loom_when_configuration_is_loom_managed() {
    let state = AppState::new_with_configuration(
        InMemoryTicketStore::default(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
        ConfigurationRuntime::loom_managed_for_tests("loom://settings/tea"),
    );
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/settings")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = body_text(response).await;
    assert!(body.contains("data-configuration-source=\"loom-managed\""));
    assert!(body.contains("This Tea configuration is managed by Loom"));
    assert!(body.contains("href=\"loom://settings/tea\""));
    assert!(body.contains("Open Loom Tea settings"));
    assert!(body.contains("disabled"));
    assert!(!body.contains("http://127.0.0.1:8765"));
}

#[test]
fn settings_page_omits_unsafe_loom_panel_links() {
    let configuration =
        ConfigurationRuntime::loom_managed_for_tests("javascript:alert(document.domain)")
            .response()
            .unwrap();

    let body = render_settings_page(&configuration);
    assert!(body.contains("This Tea configuration is managed by Loom"));
    assert!(body.contains("Loom did not provide a safe settings link."));
    assert!(!body.contains("javascript:"));
    assert!(!body.contains("<a class=\"primary-link\""));
}

#[test]
fn settings_page_redacts_unauthenticated_runtime_details() {
    let configuration = ConfigurationResponse {
        configuration_source: ConfigurationSource::Fallback,
        configuration: ConfigurationDetails {
            owner: ConfigurationOwner::Tea,
            local_config_path: Some("C:\\secret\\tea\\config.json".to_string()),
            loom_base_url: Some("http://internal-loom:8765".to_string()),
            loom_panel_url: None,
            reason: Some("connection failed with internal credential metadata".to_string()),
        },
        config: TeaConfiguration::default(),
    };

    let body = render_settings_page(&configuration);
    assert!(body.contains("Loom configuration discovery failed"));
    assert!(!body.contains("C:\\secret\\tea\\config.json"));
    assert!(!body.contains("http://internal-loom:8765"));
    assert!(!body.contains("internal credential metadata"));
}

#[tokio::test]
async fn create_ticket_uses_configured_human_default_policy() {
    let mut app = test_router();
    let config_response = app
        .call(
            Request::builder()
                .method("PUT")
                .uri("/v1/configuration")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "notifications_enabled": true,
                        "human_ticket_default_approval_policy": "manual_only",
                        "hook_ticket_default_approval_policy": "plan_only"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(config_response.status(), StatusCode::OK);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Configured","description":"Use configured policy default"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(ticket.approval_policy, tea_core::ApprovalPolicy::ManualOnly);
    assert!(ticket.labels.contains(&"policy:manual-only".to_string()));
}

#[tokio::test]
async fn create_ticket_honors_requested_approval_policy() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Explicit policy",
                        "description": "Operator picked a policy on create",
                        "approval_policy": "manual_only"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(ticket.approval_policy, tea_core::ApprovalPolicy::ManualOnly);
}

#[tokio::test]
async fn create_ticket_honors_requested_priority_and_labels() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Prioritized",
                        "description": "Operator set priority and labels on create",
                        "priority": "high",
                        "labels": ["area:desktop", "  needs-triage  ", "area:desktop", ""]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(ticket.priority, "high");
    assert!(ticket.labels.contains(&"area:desktop".to_string()));
    assert!(ticket.labels.contains(&"needs-triage".to_string()));
    // Trimmed duplicates and blank labels are dropped.
    assert_eq!(
        ticket
            .labels
            .iter()
            .filter(|label| label.as_str() == "area:desktop")
            .count(),
        1
    );
    assert!(!ticket.labels.iter().any(|label| label.is_empty()));
    // Source and policy labels are still present.
    assert!(ticket.labels.iter().any(|label| label == "source:human"));
}

#[tokio::test]
async fn patch_ticket_edits_fields_and_preserves_system_labels() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Original title",
                        "description": "Original body",
                        "labels": ["area:auth"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("PATCH")
                .uri(format!("/v1/tickets/{}", created.id))
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Edited title",
                        "priority": "high",
                        "labels": ["area:desktop", "needs-review"]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let edited: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(edited.title, "Edited title");
    // Description was not provided, so it is unchanged.
    assert_eq!(edited.description, "Original body");
    assert_eq!(edited.priority, "high");
    // New operator labels replaced the old ones.
    assert!(edited.labels.iter().any(|label| label == "area:desktop"));
    assert!(edited.labels.iter().any(|label| label == "needs-review"));
    assert!(!edited.labels.iter().any(|label| label == "area:auth"));
    // System labels are preserved.
    assert!(edited.labels.iter().any(|label| label == "source:human"));
    assert!(edited
        .labels
        .iter()
        .any(|label| label.starts_with("policy:")));
}

#[tokio::test]
async fn patch_ticket_rejects_terminal_ticket() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title": "To cancel", "description": "Will be cancelled"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/cancel", created.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .call(
            Request::builder()
                .method("PATCH")
                .uri(format!("/v1/tickets/{}", created.id))
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(json!({"title": "too late"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn create_ticket_rejects_invalid_approval_policy() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Bad policy",
                        "description": "Invalid approval policy value",
                        "approval_policy": "not_a_real_policy"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn hook_intake_uses_configured_hook_default_policy_label() {
    let mut app = test_router();
    let config_response = app
        .call(
            Request::builder()
                .method("PUT")
                .uri("/v1/configuration")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "notifications_enabled": true,
                        "human_ticket_default_approval_policy": "human_before_execute",
                        "hook_ticket_default_approval_policy": "human_before_execute"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(config_response.status(), StatusCode::OK);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/intake/hook")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "source":"hook",
                        "text":"Please analyze current failure",
                        "context":{
                            "active_window":"PowerShell",
                            "selection_text":"cargo test failed",
                            "ocr_text":null,
                            "screenshot_ref":null,
                            "cwd":"C:\\repo",
                            "app":"terminal"
                        },
                        "attachments":[]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        ticket.approval_policy,
        tea_core::ApprovalPolicy::HumanBeforeExecute
    );
    assert!(ticket
        .labels
        .contains(&"policy:human-before-execute".to_string()));
    assert!(!ticket.labels.contains(&"policy:plan-only".to_string()));
    assert!(ticket.labels.contains(&"context:untrusted".to_string()));
}

#[tokio::test]
async fn create_ticket_requires_auth() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Smoke","description":"Body"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[test]
fn idempotency_key_header_must_be_unambiguous() {
    let mut headers = HeaderMap::new();
    headers.append(
        "idempotency-key",
        axum::http::HeaderValue::from_static("first"),
    );
    headers.append(
        "idempotency-key",
        axum::http::HeaderValue::from_static("second"),
    );

    let error =
        idempotency_request(&headers, HUMAN_CREATE_IDEMPOTENCY_SCOPE, &json!({})).unwrap_err();

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error.message,
        "Idempotency-Key must be supplied exactly once"
    );
}

#[tokio::test]
async fn create_and_list_ticket() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Smoke","description":"Create a safe plan"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .call(
            Request::builder()
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn create_ticket_idempotency_replays_and_rejects_changed_payloads() {
    let store = InMemoryTicketStore::default();
    let mut app = router(AppState::new(
        store.clone(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    ));
    let original_body = json!({
        "title": "Idempotent API ticket",
        "description": "Repeated authenticated requests must return one ticket."
    })
    .to_string();

    let first = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .header("idempotency-key", "api-request-1")
                .body(Body::from(original_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Ticket = serde_json::from_slice(&body_bytes(first).await).unwrap();

    let replay = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .header("idempotency-key", "api-request-1")
                .body(Body::from(original_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: Ticket = serde_json::from_slice(&body_bytes(replay).await).unwrap();

    assert_eq!(replay, first);
    assert_eq!(store.list_tickets().await.unwrap().len(), 1);
    assert_eq!(store.ticket_events(&first.id).await.unwrap().len(), 1);

    let conflict = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .header("idempotency-key", "api-request-1")
                .body(Body::from(
                    json!({
                        "title": "Changed idempotent API ticket",
                        "description": "The same key cannot identify a different request."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let body: Value = serde_json::from_slice(&body_bytes(conflict).await).unwrap();
    assert_eq!(
        body["error"],
        "idempotency key was already used with a different request"
    );
    assert_eq!(store.list_tickets().await.unwrap().len(), 1);
}

#[tokio::test]
async fn create_ticket_rejects_invalid_idempotency_keys() {
    for key in [
        "".to_string(),
        "contains space".to_string(),
        "a".repeat(256),
    ] {
        let response = test_router()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/tickets")
                    .header("authorization", "Bearer dev-token")
                    .header("content-type", "application/json")
                    .header("idempotency-key", key)
                    .body(Body::from(
                        json!({
                            "title": "Invalid idempotency key",
                            "description": "Invalid keys must fail before creating a ticket."
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}

#[tokio::test]
async fn create_idempotency_keys_are_scoped_between_human_and_hook_routes() {
    let store = InMemoryTicketStore::default();
    let mut app = router(AppState::new(
        store.clone(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    ));
    let human = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .header("idempotency-key", "shared-route-key")
                .body(Body::from(
                    json!({
                        "title": "Human scoped ticket",
                        "description": "Human and Hook routes use independent key scopes."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(human.status(), StatusCode::OK);
    let human: Ticket = serde_json::from_slice(&body_bytes(human).await).unwrap();
    let hook_body = json!({
        "source": "hook-smoke",
        "text": "Hook scoped ticket uses the same external key.",
        "context": {
            "active_window": null,
            "selection_text": null,
            "ocr_text": null,
            "screenshot_ref": null,
            "cwd": null,
            "app": "Tea"
        },
        "attachments": []
    })
    .to_string();

    let hook = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/intake/hook")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .header("idempotency-key", "shared-route-key")
                .body(Body::from(hook_body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hook.status(), StatusCode::OK);
    let hook: Ticket = serde_json::from_slice(&body_bytes(hook).await).unwrap();

    let hook_replay = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/intake/hook")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .header("idempotency-key", "shared-route-key")
                .body(Body::from(hook_body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(hook_replay.status(), StatusCode::OK);
    let hook_replay: Ticket = serde_json::from_slice(&body_bytes(hook_replay).await).unwrap();

    assert_ne!(human.id, hook.id);
    assert_eq!(hook_replay, hook);
    assert_eq!(store.list_tickets().await.unwrap().len(), 2);
}

#[tokio::test]
async fn decompose_ticket_stores_analysis_and_plan_from_one_provider_proposal() {
    let observed_store = InMemoryTicketStore::default();
    let state = AppState::new(
        observed_store.clone(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Decompose",
                        "description": "Use one BrainProvider proposal for analysis and plan."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/decompose", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["provider"]["capability"], "tea.ticket.decompose.v1");
    assert_eq!(body["analysis"]["intent"], "engineering_work_order");
    assert_eq!(
        body["analysis"]["recommended_workflow"],
        "loom.tea_ticket_decompose.v1"
    );
    assert!(body["plan"]["steps"].as_array().unwrap().len() >= 3);
    assert_eq!(body["plan"]["requires_approval_before_execute"], true);

    let stored_ticket = observed_store.get_ticket(&ticket.id).await.unwrap();
    assert_eq!(stored_ticket.status, TicketStatus::AwaitingApproval);
    let events = observed_store.ticket_events(&ticket.id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::TicketAnalyzed));
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::PlanProposed));
}

fn valid_decomposition_proposal(
    policy: ApprovalPolicy,
    risk: tea_core::RiskLevel,
    requires_approval_before_execute: bool,
) -> DecomposeTicketProposal {
    DecomposeTicketProposal {
        schema_version: 1,
        proposal_id: "proposal-review".to_string(),
        analysis: TicketAnalysis {
            intent: "review proposal policy".to_string(),
            target_components: vec!["Tea".to_string()],
            target_paths: vec![],
            constraints: vec![],
            acceptance_criteria: vec!["policy gate is preserved".to_string()],
            missing_context: vec![],
            risk_assessment: risk,
            confidence: 0.8,
            recommended_policy: policy,
            recommended_workflow: "loom.review".to_string(),
        },
        plan: Plan {
            summary: "Review the provider proposal.".to_string(),
            steps: vec![tea_core::PlanStep {
                id: "review".to_string(),
                title: "Review".to_string(),
                description: "Verify the approval gate.".to_string(),
            }],
            required_tools: vec![],
            expected_artifacts: vec![],
            validation_strategy: vec![],
            rollback_strategy: vec![],
            requires_approval_before_execute,
        },
        requires_human_review: true,
        notes: vec![],
    }
}

#[test]
fn provider_proposal_cannot_weaken_policy_or_omit_required_gate() {
    let downgrade =
        valid_decomposition_proposal(ApprovalPolicy::AlwaysAuto, tea_core::RiskLevel::Low, false);
    let error = validate_decomposition_proposal(
        &downgrade,
        ApprovalPolicy::HumanBeforeExecute,
        TicketSource::Human,
    )
    .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_GATEWAY);
    assert!(error.message.contains("weakens"));

    let completion_downgrade =
        valid_decomposition_proposal(ApprovalPolicy::AlwaysAuto, tea_core::RiskLevel::Low, false);
    let error = validate_decomposition_proposal(
        &completion_downgrade,
        ApprovalPolicy::HumanBeforeCompletion,
        TicketSource::Human,
    )
    .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_GATEWAY);
    assert!(error.message.contains("weakens"));

    let missing_gate = valid_decomposition_proposal(
        ApprovalPolicy::HumanBeforeExecute,
        tea_core::RiskLevel::Low,
        false,
    );
    let error = validate_decomposition_proposal(
        &missing_gate,
        ApprovalPolicy::HumanBeforeExecute,
        TicketSource::Human,
    )
    .unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_GATEWAY);
    assert!(error.message.contains("omits the approval gate"));

    let valid = valid_decomposition_proposal(
        ApprovalPolicy::HumanBeforeExecute,
        tea_core::RiskLevel::Low,
        true,
    );
    validate_decomposition_proposal(
        &valid,
        ApprovalPolicy::HumanBeforeExecute,
        TicketSource::Human,
    )
    .unwrap();
}

#[tokio::test]
async fn analysis_and_plan_records_are_readable_after_decompose() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "title": "Readable records",
                        "description": "Analysis and plan must be readable after decompose."
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    // Before decompose: records read back as JSON null, not 404.
    let response = app
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/tickets/{}/analysis", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body.is_null());

    let response = app
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/tickets/{}/plan", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body.is_null());

    // Generate the records.
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/decompose", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // After decompose: GET returns the stored analysis and plan.
    let response = app
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/tickets/{}/analysis", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["intent"], "engineering_work_order");

    let response = app
        .call(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/tickets/{}/plan", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["steps"].as_array().unwrap().len() >= 3);
}

#[tokio::test]
async fn analysis_and_plan_records_require_auth() {
    let app = test_router();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v1/tickets/{}/analysis", TicketId::new()))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn run_requires_approval() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Smoke","description":"Create a safe plan"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn run_rejects_needs_info_even_for_always_auto_policy() {
    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket_with_policy(
            "Missing context".to_string(),
            "The provider needs more information before execution.".to_string(),
            TicketSource::Human,
            ActorRef::human("reviewer"),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    store
        .set_analysis(
            &ticket.id,
            ActorRef::system(),
            TicketAnalysis {
                intent: "request context".to_string(),
                target_components: vec!["Tea".to_string()],
                target_paths: vec![],
                constraints: vec![],
                acceptance_criteria: vec![],
                missing_context: vec!["repository path".to_string()],
                risk_assessment: tea_core::RiskLevel::Low,
                confidence: 0.2,
                recommended_policy: ApprovalPolicy::AlwaysAuto,
                recommended_workflow: "wait".to_string(),
            },
        )
        .await
        .unwrap();
    store
        .set_plan(
            &ticket.id,
            ActorRef::system(),
            Plan {
                summary: "Wait for context.".to_string(),
                steps: vec![tea_core::PlanStep {
                    id: "wait".to_string(),
                    title: "Wait".to_string(),
                    description: "Do not execute yet.".to_string(),
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
        store.get_ticket(&ticket.id).await.unwrap().status,
        TicketStatus::NeedsInfo
    );

    let state = AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    );
    let response = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn accept_requires_run_evidence() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Review","description":"Accept only after evidence exists"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/accept", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"], "ticket transition requires evidence");
}

#[tokio::test]
async fn accept_after_run_evidence_succeeds() {
    let mut app = test_router();
    let run = create_approved_ticket_and_run(&mut app).await;

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/accept", run.ticket_id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(ticket.status, TicketStatus::Accepted);
}

#[tokio::test]
async fn close_honors_completion_approval_policy() {
    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Completion approval".to_string(),
            "Closing should require a final human decision.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .set_analysis(
            &ticket.id,
            ActorRef::system(),
            tea_core::TicketAnalysis {
                intent: "verify completion policy".to_string(),
                target_components: vec!["tea_api".to_string()],
                target_paths: vec!["Tea/crates/tea_api/src/lib.rs".to_string()],
                constraints: vec![],
                acceptance_criteria: vec!["close requires approval".to_string()],
                missing_context: vec![],
                risk_assessment: tea_core::RiskLevel::Low,
                confidence: 0.9,
                recommended_policy: tea_core::ApprovalPolicy::HumanBeforeCompletion,
                recommended_workflow: "manual close".to_string(),
            },
        )
        .await
        .unwrap();
    store
        .add_run(
            &ticket.id,
            ActorRef::loom("test-loom"),
            tea_core::Run {
                id: RunId::new(),
                ticket_id: ticket.id.clone(),
                loom_session_id: Some("test".to_string()),
                status: tea_core::RunStatus::Succeeded,
                evidence: Some(tea_core::RunEvidence {
                    summary: "done".to_string(),
                    commands: vec![],
                    artifacts: vec![],
                    risks: vec![],
                }),
            },
        )
        .await
        .unwrap();

    let observed_store = store.clone();
    let state = AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/close", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let after_close_attempt = observed_store.get_ticket(&ticket.id).await.unwrap();
    assert_ne!(after_close_attempt.status, TicketStatus::Closed);
}

#[tokio::test]
async fn hook_intake_creates_plan_only_ticket() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/intake/hook")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "source":"hook",
                        "text":"Please analyze current failure",
                        "context":{
                            "active_window":"PowerShell",
                            "selection_text":"cargo test failed",
                            "ocr_text":null,
                            "screenshot_ref":null,
                            "cwd":"C:\\repo",
                            "app":"terminal"
                        },
                        "attachments":[{
                            "kind":"screenshot",
                            "reference":"hook://capture/123"
                        }]
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert!(ticket.labels.contains(&"source:hook".to_string()));
    assert!(ticket.labels.contains(&"policy:plan-only".to_string()));
    assert!(ticket.labels.contains(&"context:untrusted".to_string()));
    assert!(ticket
        .description
        .contains("attachment[0].kind: screenshot"));
    assert!(ticket
        .description
        .contains("attachment[0].reference: hook://capture/123"));
}

#[tokio::test]
async fn ticket_policy_endpoint_updates_policy_and_appends_event() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Policy","description":"Override approval policy"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/policy", ticket.id))
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(json!({"mode":"manual_only"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let updated: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        updated.approval_policy,
        tea_core::ApprovalPolicy::ManualOnly
    );

    let events_response = app
        .call(
            Request::builder()
                .uri(format!("/v1/tickets/{}/events", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events_response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(events_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: Vec<tea_core::TicketEvent> = serde_json::from_slice(&bytes).unwrap();
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::PolicyUpdated));
}

#[tokio::test]
async fn approve_run_and_close_ticket_with_evidence() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Smoke","description":"Create a safe plan"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let approve_response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/approve", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approve_response.status(), StatusCode::OK);

    let run_response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run_response.status(), StatusCode::OK);

    let close_response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/close", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(close_response.status(), StatusCode::OK);

    let events_response = app
        .call(
            Request::builder()
                .uri(format!("/v1/tickets/{}/events", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(events_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: Vec<tea_core::TicketEvent> = serde_json::from_slice(&bytes).unwrap();
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::ApprovalGranted));
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::RunSucceeded));
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::EvidenceAttached));
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::TicketClosed));
}

#[tokio::test]
async fn concurrent_run_requests_dispatch_to_loom_only_once() {
    use axum::{
        extract::State as AxumState, routing::post, Json as AxumJson, Router as AxumRouter,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct BlockingStartState {
        calls: Arc<AtomicUsize>,
        entered: Arc<tokio::sync::Semaphore>,
        release: Arc<tokio::sync::Semaphore>,
    }

    async fn start_handler(
        AxumState(state): AxumState<BlockingStartState>,
        AxumJson(body): AxumJson<Value>,
    ) -> AxumJson<tea_core::Run> {
        let ticket: tea_core::Ticket = serde_json::from_value(body["ticket"].clone()).unwrap();
        state.calls.fetch_add(1, Ordering::SeqCst);
        state.entered.add_permits(1);
        state.release.acquire().await.unwrap().forget();
        AxumJson(tea_core::Run {
            id: RunId::new(),
            ticket_id: ticket.id,
            loom_session_id: Some("blocked-start".to_string()),
            status: tea_core::RunStatus::Running,
            evidence: None,
        })
    }

    let loom_state = BlockingStartState {
        calls: Arc::new(AtomicUsize::new(0)),
        entered: Arc::new(tokio::sync::Semaphore::new(0)),
        release: Arc::new(tokio::sync::Semaphore::new(0)),
    };
    let loom_app = AxumRouter::new()
        .route("/v1/runs", post(start_handler))
        .with_state(loom_state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, loom_app).await.unwrap();
    });

    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket_with_policy(
            "Concurrent start".to_string(),
            "Only one overlapping request may dispatch to Loom.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    let app = router(AppState::new(
        store.clone(),
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(format!("http://{address}"), None),
        AuthConfig::new("dev-token".to_string()),
    ));
    let request = || {
        Request::builder()
            .method("POST")
            .uri(format!("/v1/tickets/{}/run", ticket.id))
            .header("authorization", "Bearer dev-token")
            .body(Body::empty())
            .unwrap()
    };

    let first = tokio::spawn(app.clone().oneshot(request()));
    loom_state.entered.acquire().await.unwrap().forget();
    let second = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        app.clone().oneshot(request()),
    )
    .await
    .expect("overlapping request should be rejected without waiting for Loom")
    .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
    assert_eq!(loom_state.calls.load(Ordering::SeqCst), 1);

    loom_state.release.add_permits(1);
    let first = first.await.unwrap().unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(store.list_runs(&ticket.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn run_stop_endpoint_stops_the_addressed_run() {
    let (mut app, store, run) = router_with_run_status(tea_core::RunStatus::Running).await;

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/runs/{}/stop", run.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let stopped: tea_core::Run = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(stopped.id, run.id);
    assert_eq!(stopped.ticket_id, run.ticket_id);
    assert_eq!(stopped.status, tea_core::RunStatus::Stopped);
    let events = store.ticket_events(&run.ticket_id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::RunStopped));
}

#[tokio::test]
async fn run_stop_rejects_mismatched_loom_response_id() {
    use axum::{
        extract::State as AxumState, routing::post, Json as AxumJson, Router as AxumRouter,
    };

    async fn stop_handler(
        AxumState(wrong_run): AxumState<tea_core::Run>,
    ) -> AxumJson<tea_core::Run> {
        let mut run = wrong_run;
        run.status = tea_core::RunStatus::Stopped;
        AxumJson(run)
    }

    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Stop mismatch".to_string(),
            "Loom must not redirect run actions to another run.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let addressed_run = tea_core::Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: Some("addressed".to_string()),
        status: tea_core::RunStatus::Running,
        evidence: None,
    };
    let wrong_run = tea_core::Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: Some("wrong".to_string()),
        status: tea_core::RunStatus::Running,
        evidence: None,
    };
    store
        .add_run(
            &ticket.id,
            ActorRef::loom("test-loom"),
            addressed_run.clone(),
        )
        .await
        .unwrap();
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), wrong_run.clone())
        .await
        .unwrap();

    let loom_app = AxumRouter::new()
        .route(
            &format!("/v1/runs/{}/stop", addressed_run.id),
            post(stop_handler),
        )
        .with_state(wrong_run.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, loom_app).await.unwrap();
    });

    let observed_store = store.clone();
    let state = AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(format!("http://{address}"), None),
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/runs/{}/stop", addressed_run.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        observed_store.get_run(&wrong_run.id).await.unwrap().status,
        tea_core::RunStatus::Running
    );
}

#[tokio::test]
async fn run_stop_rejects_loom_response_without_stopped_status() {
    use axum::{routing::post, Json as AxumJson, Router as AxumRouter};

    async fn unchanged_handler(
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> AxumJson<serde_json::Value> {
        AxumJson(body["run"].clone())
    }

    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Stop status mismatch".to_string(),
            "Loom must confirm the requested target status.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let run = tea_core::Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: Some("running".to_string()),
        status: tea_core::RunStatus::Running,
        evidence: None,
    };
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), run.clone())
        .await
        .unwrap();

    let loom_app = AxumRouter::new().route(
        &format!("/v1/runs/{}/stop", run.id),
        post(unchanged_handler),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, loom_app).await.unwrap();
    });

    let observed_store = store.clone();
    let mut app = router(AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(format!("http://{address}"), None),
        AuthConfig::new("dev-token".to_string()),
    ));
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/runs/{}/stop", run.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        observed_store.get_run(&run.id).await.unwrap().status,
        tea_core::RunStatus::Running
    );
}

#[tokio::test]
async fn run_stop_rejects_a_stale_loom_response_after_concurrent_update() {
    use axum::{
        extract::State as AxumState, routing::post, Json as AxumJson, Router as AxumRouter,
    };

    #[derive(Clone)]
    struct ConcurrentUpdate {
        store: InMemoryTicketStore,
        committed: tea_core::Run,
    }

    async fn stop_handler(
        AxumState(state): AxumState<ConcurrentUpdate>,
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> AxumJson<tea_core::Run> {
        state
            .store
            .update_run(
                &state.committed.ticket_id,
                ActorRef::loom("winning-action"),
                state.committed.clone(),
            )
            .await
            .unwrap();
        let mut stale: tea_core::Run = serde_json::from_value(body["run"].clone()).unwrap();
        stale.status = tea_core::RunStatus::Stopped;
        stale.loom_session_id = Some("stale-action".to_string());
        AxumJson(stale)
    }

    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Concurrent stop".to_string(),
            "Reject a stale Loom response without overwriting the winner.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let run = tea_core::Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: Some("original".to_string()),
        status: tea_core::RunStatus::Running,
        evidence: None,
    };
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), run.clone())
        .await
        .unwrap();
    let mut committed = run.clone();
    committed.status = tea_core::RunStatus::Stopped;
    committed.loom_session_id = Some("winning-action".to_string());

    let loom_app = AxumRouter::new()
        .route(&format!("/v1/runs/{}/stop", run.id), post(stop_handler))
        .with_state(ConcurrentUpdate {
            store: store.clone(),
            committed: committed.clone(),
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, loom_app).await.unwrap();
    });

    let observed_store = store.clone();
    let mut app = router(AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(format!("http://{address}"), None),
        AuthConfig::new("dev-token".to_string()),
    ));
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/runs/{}/stop", run.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(observed_store.get_run(&run.id).await.unwrap(), committed);
    let events = observed_store.ticket_events(&ticket.id).await.unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == tea_core::TicketEventKind::RunStopped)
            .count(),
        1
    );
}

#[tokio::test]
async fn run_retry_endpoint_retries_the_addressed_run() {
    let (mut app, store, run) = router_with_run_status(tea_core::RunStatus::Stopped).await;

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/runs/{}/retry", run.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let retrying: tea_core::Run = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(retrying.id, run.id);
    assert_eq!(retrying.ticket_id, run.ticket_id);
    assert_eq!(retrying.status, tea_core::RunStatus::Retrying);
    let events = store.ticket_events(&run.ticket_id).await.unwrap();
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::RunRetrying));
}

#[tokio::test]
async fn run_retry_rejects_mismatched_loom_response_id() {
    use axum::{
        extract::State as AxumState, routing::post, Json as AxumJson, Router as AxumRouter,
    };

    async fn retry_handler(
        AxumState(wrong_run): AxumState<tea_core::Run>,
    ) -> AxumJson<tea_core::Run> {
        let mut run = wrong_run;
        run.status = tea_core::RunStatus::Retrying;
        AxumJson(run)
    }

    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Retry mismatch".to_string(),
            "Loom must not redirect run actions to another run.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let addressed_run = tea_core::Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: Some("addressed".to_string()),
        status: tea_core::RunStatus::Stopped,
        evidence: None,
    };
    let wrong_run = tea_core::Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: Some("wrong".to_string()),
        status: tea_core::RunStatus::Stopped,
        evidence: None,
    };
    store
        .add_run(
            &ticket.id,
            ActorRef::loom("test-loom"),
            addressed_run.clone(),
        )
        .await
        .unwrap();
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), wrong_run.clone())
        .await
        .unwrap();

    let loom_app = AxumRouter::new()
        .route(
            &format!("/v1/runs/{}/retry", addressed_run.id),
            post(retry_handler),
        )
        .with_state(wrong_run.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, loom_app).await.unwrap();
    });

    let observed_store = store.clone();
    let state = AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(format!("http://{address}"), None),
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/runs/{}/retry", addressed_run.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        observed_store.get_run(&wrong_run.id).await.unwrap().status,
        tea_core::RunStatus::Stopped
    );
}

#[tokio::test]
async fn succeeded_run_rejects_stop_and_retry_before_remote_loom_calls() {
    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Terminal run".to_string(),
            "A succeeded run must retain its outcome and evidence.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let run = tea_core::Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: Some("succeeded".to_string()),
        status: tea_core::RunStatus::Succeeded,
        evidence: Some(tea_core::RunEvidence {
            summary: "verified outcome".to_string(),
            commands: vec![],
            artifacts: vec!["evidence.json".to_string()],
            risks: vec![],
        }),
    };
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), run.clone())
        .await
        .unwrap();
    let observed_store = store.clone();
    let mut app = router(AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new("http://127.0.0.1:9".to_string(), None),
        AuthConfig::new("dev-token".to_string()),
    ));

    for action in ["stop", "retry"] {
        let response = app
            .call(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/runs/{}/{action}", run.id))
                    .header("authorization", "Bearer dev-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    assert_eq!(observed_store.get_run(&run.id).await.unwrap(), run);
}

#[tokio::test]
async fn closed_ticket_rejects_mutation_with_conflict() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Closed","description":"Finish and freeze this work order"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    app.call(
        Request::builder()
            .method("POST")
            .uri(format!("/v1/tickets/{}/approve", ticket.id))
            .header("authorization", "Bearer dev-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    app.call(
        Request::builder()
            .method("POST")
            .uri(format!("/v1/tickets/{}/run", ticket.id))
            .header("authorization", "Bearer dev-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    app.call(
        Request::builder()
            .method("POST")
            .uri(format!("/v1/tickets/{}/close", ticket.id))
            .header("authorization", "Bearer dev-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/comments", ticket.id))
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(json!({"body":"late mutation"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn cancel_ticket_endpoint_sets_cancelled_and_blocks_mutations() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Cancel","description":"Cancel this work order"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/cancel", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let cancelled: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(cancelled.status, TicketStatus::Cancelled);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/comments", ticket.id))
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(json!({"body":"late mutation"}).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let events_response = app
        .call(
            Request::builder()
                .uri(format!("/v1/tickets/{}/events", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(events_response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(events_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let events: Vec<tea_core::TicketEvent> = serde_json::from_slice(&bytes).unwrap();
    assert!(events
        .iter()
        .any(|event| event.kind == tea_core::TicketEventKind::TicketCancelled));
}

#[tokio::test]
async fn closed_ticket_analyze_rejects_before_remote_ai_call() {
    let store = InMemoryTicketStore::default();
    let ticket = create_closed_ticket_in_store(&store, false).await;
    let state = AppState::new(
        store,
        tea_brain::LoomCapabilityBrainProvider::new(
            "http://127.0.0.1:9".to_string(),
            Some("brain-token".to_string()),
        ),
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/analyze", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn closed_ticket_run_rejects_before_remote_loom_call() {
    let store = InMemoryTicketStore::default();
    let ticket = create_closed_ticket_in_store(&store, true).await;
    let state = AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(
            "http://127.0.0.1:9".to_string(),
            Some("loom-token".to_string()),
        ),
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn blocked_ticket_run_rejects_before_remote_loom_call() {
    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Blocked".to_string(),
            "Rejected tickets must not start remote Loom runs.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    store
        .set_approval_policy(
            &ticket.id,
            ActorRef::human("vmjcv"),
            ApprovalPolicy::AlwaysAuto,
        )
        .await
        .unwrap();
    store
        .reject_approval(
            &ticket.id,
            ActorRef::human("vmjcv"),
            "Rejected by reviewer".to_string(),
        )
        .await
        .unwrap();
    let state = AppState::new(
        store,
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(
            "http://127.0.0.1:9".to_string(),
            Some("loom-token".to_string()),
        ),
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn export_markdown_returns_run_evidence() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Smoke","description":"Create a safe plan"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    app.call(
        Request::builder()
            .method("POST")
            .uri(format!("/v1/tickets/{}/approve", ticket.id))
            .header("authorization", "Bearer dev-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    app.call(
        Request::builder()
            .method("POST")
            .uri(format!("/v1/tickets/{}/run", ticket.id))
            .header("authorization", "Bearer dev-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    let response = app
        .call(
            Request::builder()
                .uri(format!("/v1/tickets/{}/export/markdown", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(body.contains("mock loom run completed"));
}

#[tokio::test]
async fn comments_endpoint_and_exports_return_review_comment_bodies() {
    let mut app = test_router();
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Commented","description":"Ticket with review comments"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/comments", ticket.id))
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"body":"Manual review comment must be exportable"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let comments_response = app
        .call(
            Request::builder()
                .uri(format!("/v1/tickets/{}/comments", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(comments_response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(comments_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let comments: Vec<tea_core::TicketComment> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].body, "Manual review comment must be exportable");

    let export_response = app
        .call(
            Request::builder()
                .uri(format!("/v1/tickets/{}/export/json", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(export_response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(export_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let exported: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        exported["comments"][0]["body"],
        "Manual review comment must be exportable"
    );

    let markdown_response = app
        .call(
            Request::builder()
                .uri(format!("/v1/tickets/{}/export/markdown", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(markdown_response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(markdown_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let markdown = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(markdown.contains("## Comments"));
    assert!(markdown.contains("Manual review comment must be exportable"));
}

#[tokio::test]
async fn remote_ai_failure_returns_bad_gateway() {
    let state = AppState::new(
        InMemoryTicketStore::default(),
        tea_brain::LoomCapabilityBrainProvider::new(
            "http://127.0.0.1:9".to_string(),
            Some("brain-token".to_string()),
        ),
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Remote AI","description":"Analyze through remote brain"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/analyze", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn remote_loom_failure_returns_bad_gateway() {
    let state = AppState::new(
        InMemoryTicketStore::default(),
        tea_brain::TemplateBrainProvider,
        tea_loom::HttpLoomClient::new(
            "http://127.0.0.1:9".to_string(),
            Some("loom-token".to_string()),
        ),
        AuthConfig::new("dev-token".to_string()),
    );
    let mut app = router(state);
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Remote Loom","description":"Execute through remote loom"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    app.call(
        Request::builder()
            .method("POST")
            .uri(format!("/v1/tickets/{}/approve", ticket.id))
            .header("authorization", "Bearer dev-token")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();

    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
}

async fn create_closed_ticket_in_store(
    store: &InMemoryTicketStore,
    approved: bool,
) -> tea_core::Ticket {
    let ticket = store
        .create_ticket(
            "Closed".to_string(),
            "Closed before remote side effects.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    if approved {
        store
            .grant_approval(&ticket.id, ActorRef::human("vmjcv"))
            .await
            .unwrap();
    }
    store
        .add_run(
            &ticket.id,
            ActorRef::loom("test-loom"),
            tea_core::Run {
                id: RunId::new(),
                ticket_id: ticket.id.clone(),
                loom_session_id: Some("test".to_string()),
                status: tea_core::RunStatus::Succeeded,
                evidence: Some(tea_core::RunEvidence {
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
        .close_ticket(&ticket.id, ActorRef::human("vmjcv"))
        .await
        .unwrap()
}

async fn router_with_run_status(
    status: tea_core::RunStatus,
) -> (Router, InMemoryTicketStore, tea_core::Run) {
    let store = InMemoryTicketStore::default();
    let ticket = store
        .create_ticket(
            "Run action".to_string(),
            "Exercise a stateful run-level action endpoint.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        )
        .await
        .unwrap();
    let run = tea_core::Run {
        id: RunId::new(),
        ticket_id: ticket.id.clone(),
        loom_session_id: Some("stateful-run".to_string()),
        status,
        evidence: None,
    };
    store
        .add_run(&ticket.id, ActorRef::loom("test-loom"), run.clone())
        .await
        .unwrap();
    let app = router(AppState::new(
        store.clone(),
        tea_brain::TemplateBrainProvider,
        tea_loom::MockLoomClient,
        AuthConfig::new("dev-token".to_string()),
    ));
    (app, store, run)
}

async fn create_approved_ticket_and_run(app: &mut Router) -> tea_core::Run {
    let response = app
        .call(
            Request::builder()
                .method("POST")
                .uri("/v1/tickets")
                .header("authorization", "Bearer dev-token")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"title":"Run action","description":"Exercise run-level action endpoint"})
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let ticket: tea_core::Ticket = serde_json::from_slice(&bytes).unwrap();

    let approve_response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/approve", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(approve_response.status(), StatusCode::OK);

    let run_response = app
        .call(
            Request::builder()
                .method("POST")
                .uri(format!("/v1/tickets/{}/run", ticket.id))
                .header("authorization", "Bearer dev-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(run_response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(run_response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn body_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn body_bytes(response: Response) -> axum::body::Bytes {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
}

fn temp_store_path(prefix: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("current time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.sqlite", std::process::id()))
}

fn temp_config_path(prefix: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("current time should be after unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}.json", std::process::id()))
}

fn config_lock_path_for_test(path: &std::path::Path) -> std::path::PathBuf {
    let mut file_name = path.file_name().unwrap_or_default().to_os_string();
    file_name.push(".lock");
    path.with_file_name(file_name)
}
