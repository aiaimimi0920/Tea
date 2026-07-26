#![forbid(unsafe_code)]

use async_trait::async_trait;
use tea_config::{LoomManagedDocumentMetadata, LoomManagedTeaConfiguration, TeaConfiguration};
use tea_core::{Run, RunEvidence, RunId, RunStatus, Ticket};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LoomError {
    #[error("Loom request failed: {0}")]
    Request(#[from] reqwest::Error),
}

pub type LoomResult<T> = Result<T, LoomError>;

#[async_trait]
pub trait LoomClient: Clone + Send + Sync + 'static {
    async fn start_run(&self, ticket: &Ticket) -> LoomResult<Run>;
    async fn stop_run(&self, run: &Run) -> LoomResult<Run>;
    async fn retry_run(&self, run: &Run) -> LoomResult<Run>;
    async fn read_tea_configuration(&self) -> LoomResult<LoomManagedTeaConfiguration>;
    async fn write_tea_configuration(
        &self,
        expected_revision: u64,
        config: &TeaConfiguration,
    ) -> LoomResult<LoomManagedTeaConfiguration>;
}

#[derive(Debug, Clone, Default)]
pub struct MockLoomClient;

#[async_trait]
impl LoomClient for MockLoomClient {
    async fn start_run(&self, ticket: &Ticket) -> LoomResult<Run> {
        Ok(Run {
            id: RunId::new(),
            ticket_id: ticket.id.clone(),
            loom_session_id: Some("mock-loom-session".to_string()),
            status: RunStatus::Succeeded,
            evidence: Some(RunEvidence {
                summary: "mock loom run completed".to_string(),
                commands: vec![],
                artifacts: vec!["mock evidence".to_string()],
                risks: vec![],
            }),
        })
    }

    async fn stop_run(&self, run: &Run) -> LoomResult<Run> {
        let mut stopped = run.clone();
        stopped.status = RunStatus::Stopped;
        Ok(stopped)
    }

    async fn retry_run(&self, run: &Run) -> LoomResult<Run> {
        let mut retrying = run.clone();
        retrying.status = RunStatus::Retrying;
        Ok(retrying)
    }

    async fn read_tea_configuration(&self) -> LoomResult<LoomManagedTeaConfiguration> {
        Ok(mock_tea_configuration(
            TeaConfiguration::default(),
            1,
            false,
        ))
    }

    async fn write_tea_configuration(
        &self,
        expected_revision: u64,
        config: &TeaConfiguration,
    ) -> LoomResult<LoomManagedTeaConfiguration> {
        Ok(mock_tea_configuration(
            config.clone(),
            expected_revision.saturating_add(1),
            false,
        ))
    }
}

#[derive(Debug, Clone)]
pub struct HttpLoomClient {
    base_url: String,
    auth_token: Option<String>,
    http: reqwest::Client,
}

impl HttpLoomClient {
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
struct StartRunRequest<'a> {
    ticket: &'a Ticket,
}

#[derive(Debug, serde::Serialize)]
struct RunActionRequest<'a> {
    run: &'a Run,
}

#[derive(Debug, serde::Serialize)]
struct PutTeaConfigurationRequest<'a> {
    expected_revision: u64,
    config: &'a TeaConfiguration,
}

#[async_trait]
impl LoomClient for HttpLoomClient {
    async fn start_run(&self, ticket: &Ticket) -> LoomResult<Run> {
        let response = self
            .authorize(
                self.http
                    .post(self.url("/v1/runs"))
                    .json(&StartRunRequest { ticket }),
            )
            .send()
            .await?
            .error_for_status()?
            .json::<Run>()
            .await?;
        Ok(response)
    }

    async fn stop_run(&self, run: &Run) -> LoomResult<Run> {
        let response = self
            .authorize(
                self.http
                    .post(self.url(&format!("/v1/runs/{}/stop", run.id)))
                    .json(&RunActionRequest { run }),
            )
            .send()
            .await?
            .error_for_status()?
            .json::<Run>()
            .await?;
        Ok(response)
    }

    async fn retry_run(&self, run: &Run) -> LoomResult<Run> {
        let response = self
            .authorize(
                self.http
                    .post(self.url(&format!("/v1/runs/{}/retry", run.id)))
                    .json(&RunActionRequest { run }),
            )
            .send()
            .await?
            .error_for_status()?
            .json::<Run>()
            .await?;
        Ok(response)
    }

    async fn read_tea_configuration(&self) -> LoomResult<LoomManagedTeaConfiguration> {
        let response = self
            .authorize(self.http.get(self.url("/v1/configuration/apps/tea")))
            .send()
            .await?
            .error_for_status()?
            .json::<LoomManagedTeaConfiguration>()
            .await?;
        Ok(response)
    }

    async fn write_tea_configuration(
        &self,
        expected_revision: u64,
        config: &TeaConfiguration,
    ) -> LoomResult<LoomManagedTeaConfiguration> {
        let response = self
            .authorize(self.http.put(self.url("/v1/configuration/apps/tea")).json(
                &PutTeaConfigurationRequest {
                    expected_revision,
                    config,
                },
            ))
            .send()
            .await?
            .error_for_status()?
            .json::<LoomManagedTeaConfiguration>()
            .await?;
        Ok(response)
    }
}

#[derive(Debug, Clone)]
pub enum RuntimeLoomClient {
    Mock(MockLoomClient),
    Http(HttpLoomClient),
}

impl RuntimeLoomClient {
    pub fn mock() -> Self {
        Self::Mock(MockLoomClient)
    }

    pub fn http(base_url: String, auth_token: Option<String>) -> Self {
        Self::Http(HttpLoomClient::new(base_url, auth_token))
    }
}

#[async_trait]
impl LoomClient for RuntimeLoomClient {
    async fn start_run(&self, ticket: &Ticket) -> LoomResult<Run> {
        match self {
            Self::Mock(client) => client.start_run(ticket).await,
            Self::Http(client) => client.start_run(ticket).await,
        }
    }

    async fn stop_run(&self, run: &Run) -> LoomResult<Run> {
        match self {
            Self::Mock(client) => client.stop_run(run).await,
            Self::Http(client) => client.stop_run(run).await,
        }
    }

    async fn retry_run(&self, run: &Run) -> LoomResult<Run> {
        match self {
            Self::Mock(client) => client.retry_run(run).await,
            Self::Http(client) => client.retry_run(run).await,
        }
    }

    async fn read_tea_configuration(&self) -> LoomResult<LoomManagedTeaConfiguration> {
        match self {
            Self::Mock(client) => client.read_tea_configuration().await,
            Self::Http(client) => client.read_tea_configuration().await,
        }
    }

    async fn write_tea_configuration(
        &self,
        expected_revision: u64,
        config: &TeaConfiguration,
    ) -> LoomResult<LoomManagedTeaConfiguration> {
        match self {
            Self::Mock(client) => {
                client
                    .write_tea_configuration(expected_revision, config)
                    .await
            }
            Self::Http(client) => {
                client
                    .write_tea_configuration(expected_revision, config)
                    .await
            }
        }
    }
}

fn mock_tea_configuration(
    config: TeaConfiguration,
    revision: u64,
    created: bool,
) -> LoomManagedTeaConfiguration {
    LoomManagedTeaConfiguration {
        app: "tea".to_string(),
        owner: "loom".to_string(),
        source: tea_config::ConfigurationSource::LoomManaged,
        writable: true,
        created,
        document: LoomManagedDocumentMetadata {
            document_version: 1,
            schema_version: 1,
            revision,
            updated_at: "1970-01-01T00:00:00Z".to_string(),
        },
        config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tea_core::{ActorRef, TicketId, TicketSource};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn mock_run_succeeds_with_evidence() {
        let ticket = Ticket::new(
            TicketId::new(),
            "Run".to_string(),
            "Run this safely".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );
        let run = MockLoomClient.start_run(&ticket).await.unwrap();
        assert_eq!(run.status, RunStatus::Succeeded);
        assert_eq!(
            run.evidence.unwrap().summary,
            "mock loom run completed".to_string()
        );
    }

    #[tokio::test]
    async fn http_loom_client_posts_ticket_with_bearer_and_parses_run() {
        use axum::{extract::State, http::HeaderMap, routing::post, Json, Router};
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Capture {
            authorization: Arc<Mutex<Option<String>>>,
            body: Arc<Mutex<Option<serde_json::Value>>>,
        }

        async fn start_run_handler(
            State(capture): State<Capture>,
            headers: HeaderMap,
            Json(body): Json<serde_json::Value>,
        ) -> Json<Run> {
            *capture.authorization.lock().unwrap() = headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string);
            let ticket_id = serde_json::from_value(body["ticket"]["id"].clone()).unwrap();
            *capture.body.lock().unwrap() = Some(body);
            Json(Run {
                id: RunId::new(),
                ticket_id,
                loom_session_id: Some("remote-session".to_string()),
                status: RunStatus::Succeeded,
                evidence: Some(RunEvidence {
                    summary: "remote loom run completed".to_string(),
                    commands: vec!["cargo test".to_string()],
                    artifacts: vec!["evidence.md".to_string()],
                    risks: vec![],
                }),
            })
        }

        let capture = Capture::default();
        let app = Router::new()
            .route("/v1/runs", post(start_run_handler))
            .with_state(capture.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let ticket = Ticket::new(
            TicketId::new(),
            "Run".to_string(),
            "Ask a remote Loom daemon to execute this ticket.".to_string(),
            TicketSource::Human,
            ActorRef::human("vmjcv"),
        );
        let client =
            HttpLoomClient::new(format!("http://{address}"), Some("loom-token".to_string()));

        let run = client.start_run(&ticket).await.unwrap();

        assert_eq!(run.loom_session_id.as_deref(), Some("remote-session"));
        assert_eq!(
            run.evidence.unwrap().summary,
            "remote loom run completed".to_string()
        );
        assert_eq!(
            capture.authorization.lock().unwrap().as_deref(),
            Some("Bearer loom-token")
        );
        let body = capture.body.lock().unwrap().clone().unwrap();
        assert_eq!(body["ticket"]["id"], serde_json::json!(ticket.id));
        assert_eq!(body["ticket"]["title"], "Run");
    }

    #[tokio::test]
    async fn http_loom_client_posts_stop_and_retry_to_run_action_routes() {
        use axum::{extract::State, routing::post, Json, Router};
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct Capture {
            paths: Arc<Mutex<Vec<String>>>,
        }

        async fn stop_handler(
            State(capture): State<Capture>,
            Json(body): Json<serde_json::Value>,
        ) -> Json<Run> {
            let mut run: Run = serde_json::from_value(body["run"].clone()).unwrap();
            capture
                .paths
                .lock()
                .unwrap()
                .push(format!("/v1/runs/{}/stop", run.id));
            run.status = RunStatus::Stopped;
            Json(run)
        }

        async fn retry_handler(
            State(capture): State<Capture>,
            Json(body): Json<serde_json::Value>,
        ) -> Json<Run> {
            let mut run: Run = serde_json::from_value(body["run"].clone()).unwrap();
            capture
                .paths
                .lock()
                .unwrap()
                .push(format!("/v1/runs/{}/retry", run.id));
            run.status = RunStatus::Retrying;
            Json(run)
        }

        let capture = Capture::default();
        let ticket_id = TicketId::new();
        let run = Run {
            id: RunId::new(),
            ticket_id: ticket_id.clone(),
            loom_session_id: Some("remote-session".to_string()),
            status: RunStatus::Running,
            evidence: None,
        };
        let app = Router::new()
            .route(&format!("/v1/runs/{}/stop", run.id), post(stop_handler))
            .route(&format!("/v1/runs/{}/retry", run.id), post(retry_handler))
            .with_state(capture.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let client = HttpLoomClient::new(format!("http://{address}"), None);

        let stopped = client.stop_run(&run).await.unwrap();
        let retrying = client.retry_run(&run).await.unwrap();

        assert_eq!(stopped.status, RunStatus::Stopped);
        assert_eq!(retrying.status, RunStatus::Retrying);
        assert_eq!(
            *capture.paths.lock().unwrap(),
            vec![
                format!("/v1/runs/{}/stop", run.id),
                format!("/v1/runs/{}/retry", run.id)
            ]
        );
        assert_eq!(stopped.ticket_id, ticket_id);
    }

    #[tokio::test]
    async fn http_loom_client_read_tea_managed_configuration() {
        use axum::{routing::get, Json, Router};
        use serde_json::json;

        async fn handler() -> Json<serde_json::Value> {
            Json(json!({
                "app": "tea",
                "owner": "loom",
                "source": "loom-managed",
                "writable": true,
                "created": true,
                "document": {
                    "document_version": 1,
                    "schema_version": 1,
                    "revision": 7,
                    "updated_at": "2026-06-10T00:00:00Z"
                },
                "config": {
                    "notifications_enabled": false,
                    "human_ticket_default_approval_policy": "manual_only",
                    "hook_ticket_default_approval_policy": "plan_only"
                }
            }))
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route("/v1/configuration/apps/tea", get(handler)),
            )
            .await
            .unwrap();
        });

        let client = HttpLoomClient::new(format!("http://{address}"), None);
        let config = client.read_tea_configuration().await.unwrap();

        assert_eq!(config.document.revision, 7);
        assert!(!config.config.notifications_enabled);
        server.abort();
    }
}
