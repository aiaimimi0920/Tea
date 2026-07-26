#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use tea_api::{AppState, AuthConfig, ConfigurationRuntime};
use tea_brain::RuntimeTeaBrainProvider;
use tea_config::{
    decode_local_config, resolve_configuration_ownership, ConfigurationDiscovery,
    ConfigurationSource, LoomConfigurationClaim, TeaConfiguration,
};
use tea_loom::{LoomClient, RuntimeLoomClient};
use tea_store::RuntimeTicketStore;

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
    let config_path = tea_config_path(cli.config_path.as_deref());
    let local_config = read_local_config(&config_path)?;
    let store = match optional_arg(cli.store_path.as_deref()) {
        Some(path) => RuntimeTicketStore::sqlite(path)?,
        None => RuntimeTicketStore::memory(),
    };

    let loom_base_url = optional_arg(cli.loom_base_url.as_deref()).map(ToOwned::to_owned);
    let loom_auth_token = optional_arg(cli.loom_auth_token.as_deref()).map(ToOwned::to_owned);
    let brain = match loom_base_url.as_ref() {
        Some(base_url) => RuntimeTeaBrainProvider::loom(base_url.clone(), loom_auth_token.clone()),
        None => RuntimeTeaBrainProvider::template(),
    };
    let loom = match loom_base_url.as_ref() {
        Some(base_url) => RuntimeLoomClient::http(base_url.clone(), loom_auth_token.clone()),
        None => RuntimeLoomClient::mock(),
    };
    let loom_claim = match loom_base_url.as_deref() {
        Some(base_url) => {
            Some(probe_loom_configuration_claim(base_url, loom_auth_token.as_deref()).await)
        }
        None => None,
    };
    let ownership = resolve_configuration_ownership(ConfigurationDiscovery {
        local_config_path: Some(config_path.display().to_string()),
        loom_base_url: loom_base_url.clone(),
        loom_claim,
    });
    let effective_config = match ownership.source {
        ConfigurationSource::LoomManaged => match loom_base_url.as_deref() {
            Some(base_url) => match read_loom_tea_config_or_seed(
                base_url,
                loom_auth_token.as_deref(),
                &local_config,
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
            None => local_config.clone(),
        },
        ConfigurationSource::Local | ConfigurationSource::Fallback => local_config.clone(),
    };
    let configuration =
        ConfigurationRuntime::new_with_local_path(ownership, effective_config, Some(config_path));

    let state = AppState::new_with_configuration(
        store,
        brain,
        loom,
        AuthConfig::new(cli.auth_token),
        configuration,
    );
    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    println!("tea-daemon listening on http://{bind_addr}");
    axum::serve(listener, tea_api::router(state)).await?;
    Ok(())
}

fn optional_arg(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
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

fn read_local_config(path: &PathBuf) -> anyhow::Result<TeaConfiguration> {
    if !path.exists() {
        return Ok(TeaConfiguration::default());
    }
    let raw = std::fs::read_to_string(path)?;
    Ok(decode_local_config(&raw)?)
}

async fn probe_loom_configuration_claim(
    base_url: &str,
    auth_token: Option<&str>,
) -> Result<LoomConfigurationClaim, String> {
    let url = format!(
        "{}/v1/configuration/claims?app=tea",
        base_url.trim_end_matches('/')
    );
    let client = reqwest::Client::new();
    let mut request = client.get(url);
    if let Some(token) = auth_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!(
            "Loom configuration claim returned {status}: {body}"
        ));
    }
    serde_json::from_str::<LoomConfigurationClaim>(&body).map_err(|error| error.to_string())
}

async fn read_loom_tea_config_or_seed(
    base_url: &str,
    auth_token: Option<&str>,
    local_config: &TeaConfiguration,
) -> Result<TeaConfiguration, String> {
    let client =
        tea_loom::HttpLoomClient::new(base_url.to_string(), auth_token.map(ToOwned::to_owned));
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

        let seeded = read_loom_tea_config_or_seed(&format!("http://{address}"), None, &local)
            .await
            .expect("seed Loom config");

        assert_eq!(seeded, local);
        server.abort();
    }
}
