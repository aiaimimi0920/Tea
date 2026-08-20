#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::Parser;
use tea_api::{AppState, AuthConfig, ConfigurationRuntime};
use tea_brain::RuntimeTeaBrainProvider;
use tea_config::{
    read_local_config_file, resolve_configuration_ownership, ConfigurationDiscovery,
    ConfigurationSource, LoomConfigurationClaim, TeaConfiguration,
};
use tea_http_read::error_body_preview;
#[cfg(test)]
use tea_http_read::MAX_ERROR_PREVIEW_CHARS;
use tea_loom::{LoomClient, RuntimeLoomClient};
use tea_store::RuntimeTicketStore;

const LOOM_STARTUP_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const LOOM_RUNTIME_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);
const MAX_LOOM_CLAIM_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// One shared HTTP client for every Loom call this process makes (startup
/// claim/config probes and runtime brain/run requests share one connection
/// pool). Timeouts are enforced per request, so there is no client-level total
/// timeout here.
fn build_loom_http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

/// Bounded response read via the shared `tea_http_read` helper, mapped to this
/// binary's `String` error boundary.
async fn read_response_text_limited(
    response: reqwest::Response,
    max_bytes: usize,
    context: &str,
) -> Result<(reqwest::StatusCode, String), String> {
    tea_http_read::read_response_text_limited(response, max_bytes, context)
        .await
        .map_err(|error| error.to_string())
}

#[derive(Debug, Parser)]
#[command(name = "tea-daemon", about = "Tea HTTP daemon", version)]
struct Cli {
    #[arg(long, env = "TEA_BIND_ADDR", default_value = "127.0.0.1:48910")]
    bind_addr: SocketAddr,
    #[arg(long, env = "TEA_AUTH_TOKEN", default_value = "dev-token")]
    auth_token: String,
    #[arg(long, env = "TEA_STORE_PATH")]
    store_path: Option<String>,
    #[arg(long, env = "TEA_CONFIG_PATH")]
    config_path: Option<String>,
    #[arg(long, env = "TEA_LOOM_BASE_URL")]
    loom_base_url: Option<String>,
    #[arg(long, env = "TEA_LOOM_AUTH_TOKEN")]
    loom_auth_token: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let bind_addr = cli.bind_addr;
    let auth_token = validate_auth_configuration(bind_addr, &cli.auth_token)?;
    let config_path = tea_config_path(cli.config_path.as_deref());
    let local_config = read_local_config(&config_path)?;
    let store = match optional_arg(cli.store_path.as_deref()) {
        Some(path) => RuntimeTicketStore::sqlite(path)?,
        None => RuntimeTicketStore::memory(),
    };

    let loom_base_url = optional_arg(cli.loom_base_url.as_deref()).map(ToOwned::to_owned);
    let loom_auth_token = optional_arg(cli.loom_auth_token.as_deref()).map(ToOwned::to_owned);
    let loom_http = match loom_base_url.as_ref() {
        Some(_) => Some(build_loom_http_client()?),
        None => None,
    };
    let brain = match (loom_base_url.as_ref(), loom_http.as_ref()) {
        (Some(base_url), Some(http)) => RuntimeTeaBrainProvider::loom_with_client(
            base_url.clone(),
            loom_auth_token.clone(),
            http.clone(),
            LOOM_RUNTIME_REQUEST_TIMEOUT,
        ),
        _ => RuntimeTeaBrainProvider::template(),
    };
    let loom = match (loom_base_url.as_ref(), loom_http.as_ref()) {
        (Some(base_url), Some(http)) => RuntimeLoomClient::http_with_client(
            base_url.clone(),
            loom_auth_token.clone(),
            http.clone(),
            LOOM_RUNTIME_REQUEST_TIMEOUT,
        ),
        _ => RuntimeLoomClient::mock(),
    };
    let loom_claim = match (loom_base_url.as_deref(), loom_http.as_ref()) {
        (Some(base_url), Some(http)) => {
            Some(probe_loom_configuration_claim(base_url, loom_auth_token.as_deref(), http).await)
        }
        _ => None,
    };
    let ownership = resolve_configuration_ownership(ConfigurationDiscovery {
        local_config_path: Some(config_path.display().to_string()),
        loom_base_url: loom_base_url.clone(),
        loom_claim,
    });
    let effective_config = match ownership.source {
        ConfigurationSource::LoomManaged => match (loom_base_url.as_deref(), loom_http.as_ref()) {
            (Some(base_url), Some(http)) => match read_loom_tea_config_or_seed(
                base_url,
                loom_auth_token.as_deref(),
                &local_config,
                http,
            )
            .await
            {
                Ok(config) => config,
                Err(reason) => {
                    eprintln!(
                        "Tea Loom-managed config read failed; using read-only fallback: {reason}"
                    );
                    local_config.clone()
                }
            },
            _ => local_config.clone(),
        },
        ConfigurationSource::Local | ConfigurationSource::Fallback => local_config.clone(),
    };
    let configuration =
        ConfigurationRuntime::new_with_local_path(ownership, effective_config, Some(config_path));

    let state = AppState::new_with_configuration(
        store,
        brain,
        loom,
        AuthConfig::new(auth_token),
        configuration,
    );
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    println!("tea-daemon listening on http://{bind_addr}");
    axum::serve(listener, tea_api::router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

/// Resolves when the process receives Ctrl+C (or SIGTERM on unix), letting
/// axum drain in-flight requests instead of aborting them.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl+C signal handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

fn optional_arg(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn validate_auth_configuration(bind_addr: SocketAddr, auth_token: &str) -> anyhow::Result<String> {
    let token = auth_token.trim();
    if token.is_empty() {
        anyhow::bail!("TEA_AUTH_TOKEN must not be empty");
    }
    if !bind_addr.ip().is_loopback() && token == "dev-token" {
        anyhow::bail!(
            "refusing to bind Tea to non-loopback address {bind_addr} with the default dev-token; set a strong TEA_AUTH_TOKEN"
        );
    }
    Ok(token.to_string())
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .and_then(|value| optional_arg(Some(&value)).map(ToOwned::to_owned))
}

fn tea_config_path(config_path: Option<&str>) -> PathBuf {
    if let Some(path) = optional_arg(config_path) {
        return PathBuf::from(path);
    }
    if let Some(appdata) = optional_env("APPDATA") {
        return PathBuf::from(appdata)
            .join("Neuro")
            .join("tea")
            .join("config.json");
    }
    PathBuf::from(".runtime")
        .join("neuro")
        .join("tea")
        .join("config.json")
}

fn read_local_config(path: &Path) -> anyhow::Result<TeaConfiguration> {
    Ok(read_local_config_file(path)?.unwrap_or_default())
}

async fn probe_loom_configuration_claim(
    base_url: &str,
    auth_token: Option<&str>,
    client: &reqwest::Client,
) -> Result<LoomConfigurationClaim, String> {
    let url = format!(
        "{}/v1/configuration/claims?app=tea",
        base_url.trim_end_matches('/')
    );
    let mut request = client.get(url).timeout(LOOM_STARTUP_REQUEST_TIMEOUT);
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let (status, body) = read_response_text_limited(
        response,
        MAX_LOOM_CLAIM_RESPONSE_BYTES,
        "Loom configuration claim",
    )
    .await?;
    if !status.is_success() {
        return Err(format!(
            "Loom configuration claim returned {status}: {}",
            error_body_preview(&body)
        ));
    }
    serde_json::from_str::<LoomConfigurationClaim>(&body).map_err(|error| error.to_string())
}

async fn read_loom_tea_config_or_seed(
    base_url: &str,
    auth_token: Option<&str>,
    local_config: &TeaConfiguration,
    http: &reqwest::Client,
) -> Result<TeaConfiguration, String> {
    let client = tea_loom::HttpLoomClient::new_with_client(
        base_url.to_string(),
        auth_token.map(ToOwned::to_owned),
        http.clone(),
        LOOM_STARTUP_REQUEST_TIMEOUT,
    );
    match client.read_tea_configuration().await {
        Ok(response) if response.created => {
            let seeded = client
                .write_tea_configuration(response.document.revision, local_config)
                .await
                .map_err(|error| error.to_string())?;
            Ok(seeded.config)
        }
        Ok(response) => Ok(response.config),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Json, Router};
    use serde_json::json;

    #[test]
    fn auth_configuration_rejects_unsafe_non_loopback_defaults() {
        assert!(
            validate_auth_configuration("0.0.0.0:48910".parse().unwrap(), "dev-token").is_err()
        );
        assert!(validate_auth_configuration("[::]:48910".parse().unwrap(), "dev-token").is_err());
        assert!(
            validate_auth_configuration("127.0.0.1:48910".parse().unwrap(), "dev-token").is_ok()
        );
        assert!(validate_auth_configuration(
            "0.0.0.0:48910".parse().unwrap(),
            "replace-with-a-strong-token"
        )
        .is_ok());
        assert!(validate_auth_configuration("127.0.0.1:48910".parse().unwrap(), "  ").is_err());
        assert_eq!(
            validate_auth_configuration(
                "127.0.0.1:48910".parse().unwrap(),
                "  replace-with-a-strong-token  "
            )
            .unwrap(),
            "replace-with-a-strong-token"
        );
    }

    #[tokio::test]
    async fn loom_created_tea_config_is_seeded_from_local_config() {
        async fn get_config() -> Json<serde_json::Value> {
            Json(json!({
                "app": "tea",
                "owner": "loom",
                "source": "loom-managed",
                "writable": true,
                "created": true,
                "document": {
                    "document_version": 1,
                    "schema_version": 1,
                    "revision": 4,
                    "updated_at": "2026-06-10T00:00:00Z"
                },
                "config": TeaConfiguration::default()
            }))
        }

        async fn put_config(Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
            Json(json!({
                "app": "tea",
                "owner": "loom",
                "source": "loom-managed",
                "writable": true,
                "created": false,
                "document": {
                    "document_version": 1,
                    "schema_version": 1,
                    "revision": 5,
                    "updated_at": "2026-06-10T00:00:01Z"
                },
                "config": body["config"].clone()
            }))
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/v1/configuration/apps/tea",
                    get(get_config).put(put_config),
                ),
            )
            .await
            .unwrap();
        });
        let local = TeaConfiguration {
            notifications_enabled: false,
            human_ticket_default_approval_policy: "manual_only".to_string(),
            hook_ticket_default_approval_policy: "plan_only".to_string(),
        };

        let seeded = read_loom_tea_config_or_seed(
            &format!("http://{address}"),
            None,
            &local,
            &build_loom_http_client().unwrap(),
        )
        .await
        .expect("seed Loom config");

        assert_eq!(seeded, local);
        server.abort();
    }

    #[tokio::test]
    async fn loom_claim_response_reader_rejects_oversized_body() {
        use axum::{body::Body, routing::get, Router};

        async fn handler() -> Body {
            Body::from("123456789")
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, Router::new().route("/", get(handler)))
                .await
                .unwrap();
        });
        let response = reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();

        let error = read_response_text_limited(response, 8, "Loom claim")
            .await
            .unwrap_err();

        assert!(error.contains("8-byte limit"));
        server.abort();
    }

    #[test]
    fn loom_claim_error_preview_is_bounded_and_single_line() {
        let body = format!("first\nsecond\t{}", "x".repeat(600));
        let preview = error_body_preview(&body);

        assert!(!preview
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '\t')));
        assert!(preview.ends_with("..."));
        assert!(preview.chars().count() <= MAX_ERROR_PREVIEW_CHARS + 3);
    }

    #[tokio::test]
    async fn loom_claim_errors_do_not_echo_unbounded_multiline_bodies() {
        use axum::{http::StatusCode, routing::get, Router};

        let body = format!("first\nsecond\t{}", "x".repeat(600));
        let app = Router::new().route(
            "/v1/configuration/claims",
            get(move || {
                let body = body.clone();
                async move { (StatusCode::INTERNAL_SERVER_ERROR, body) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let message = probe_loom_configuration_claim(
            &format!("http://{address}"),
            None,
            &build_loom_http_client().unwrap(),
        )
        .await
        .unwrap_err();

        assert!(message.contains("500 Internal Server Error"));
        assert!(!message
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '\t')));
        assert!(message.ends_with("..."));
        assert!(message.chars().count() < 700);
        server.abort();
    }
}
