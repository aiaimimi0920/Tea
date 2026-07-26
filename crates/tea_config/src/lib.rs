#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationSource {
    Local,
    LoomManaged,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationOwner {
    Tea,
    Loom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationDetails {
    pub owner: ConfigurationOwner,
    pub local_config_path: Option<String>,
    pub loom_base_url: Option<String>,
    pub loom_panel_url: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigurationOwnership {
    #[serde(rename = "configuration_source")]
    pub source: ConfigurationSource,
    #[serde(rename = "configuration")]
    pub configuration: ConfigurationDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationDiscovery {
    pub local_config_path: Option<String>,
    pub loom_base_url: Option<String>,
    pub loom_claim: Option<Result<LoomConfigurationClaim, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoomConfigurationClaim {
    pub app: String,
    pub managed: bool,
    pub panel_url: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoomManagedDocumentMetadata {
    pub document_version: u32,
    pub schema_version: u32,
    pub revision: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoomManagedTeaConfiguration {
    pub app: String,
    pub owner: String,
    pub source: ConfigurationSource,
    pub writable: bool,
    pub created: bool,
    pub document: LoomManagedDocumentMetadata,
    pub config: TeaConfiguration,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeaConfiguration {
    pub notifications_enabled: bool,
    pub human_ticket_default_approval_policy: String,
    pub hook_ticket_default_approval_policy: String,
}

impl Default for TeaConfiguration {
    fn default() -> Self {
        Self {
            notifications_enabled: true,
            human_ticket_default_approval_policy: "human_before_execute".to_string(),
            hook_ticket_default_approval_policy: "plan_only".to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct LocalConfigDocument {
    schema_version: u32,
    #[serde(flatten)]
    config: TeaConfiguration,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("encode Tea local config: {0}")]
    Encode(#[from] serde_json::Error),
}

pub fn resolve_configuration_ownership(
    discovery: ConfigurationDiscovery,
) -> ConfigurationOwnership {
    let local_config_path = discovery.local_config_path;
    let loom_base_url = discovery.loom_base_url;

    match discovery.loom_claim {
        Some(Ok(claim)) if claim.managed && has_non_empty_panel_url(&claim) => {
            ConfigurationOwnership {
                source: ConfigurationSource::LoomManaged,
                configuration: ConfigurationDetails {
                    owner: ConfigurationOwner::Loom,
                    local_config_path,
                    loom_base_url,
                    loom_panel_url: claim.panel_url,
                    reason: claim.reason,
                },
            }
        }
        Some(Ok(claim)) => ConfigurationOwnership {
            source: ConfigurationSource::Local,
            configuration: ConfigurationDetails {
                owner: ConfigurationOwner::Tea,
                local_config_path,
                loom_base_url,
                loom_panel_url: claim.panel_url,
                reason: claim.reason,
            },
        },
        Some(Err(reason)) => ConfigurationOwnership {
            source: ConfigurationSource::Fallback,
            configuration: ConfigurationDetails {
                owner: ConfigurationOwner::Tea,
                local_config_path,
                loom_base_url,
                loom_panel_url: None,
                reason: Some(reason),
            },
        },
        None => ConfigurationOwnership {
            source: ConfigurationSource::Local,
            configuration: ConfigurationDetails {
                owner: ConfigurationOwner::Tea,
                local_config_path,
                loom_base_url,
                loom_panel_url: None,
                reason: None,
            },
        },
    }
}

pub fn encode_local_config(config: &TeaConfiguration) -> Result<String, ConfigError> {
    Ok(serde_json::to_string(&LocalConfigDocument {
        schema_version: 1,
        config: config.clone(),
    })?)
}

pub fn decode_local_config(value: &str) -> Result<TeaConfiguration, ConfigError> {
    let document: LocalConfigDocument = serde_json::from_str(value)?;
    Ok(document.config)
}

fn has_non_empty_panel_url(claim: &LoomConfigurationClaim) -> bool {
    claim
        .panel_url
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_resolves_local_without_loom() {
        let ownership = resolve_configuration_ownership(ConfigurationDiscovery {
            local_config_path: Some(
                "C:\\Users\\vmjcv\\AppData\\Roaming\\Neuro\\tea\\config.json".into(),
            ),
            loom_base_url: None,
            loom_claim: None,
        });

        assert_eq!(ownership.source, ConfigurationSource::Local);
        assert_eq!(ownership.configuration.owner, ConfigurationOwner::Tea);
        assert_eq!(
            ownership.configuration.local_config_path.as_deref(),
            Some("C:\\Users\\vmjcv\\AppData\\Roaming\\Neuro\\tea\\config.json")
        );
        assert_eq!(ownership.configuration.loom_panel_url, None);
    }

    #[test]
    fn ownership_uses_loom_only_when_claim_includes_panel_url() {
        let ownership = resolve_configuration_ownership(ConfigurationDiscovery {
            local_config_path: Some("C:\\tea\\config.json".into()),
            loom_base_url: Some("http://127.0.0.1:8765".into()),
            loom_claim: Some(Ok(LoomConfigurationClaim {
                app: "tea".into(),
                managed: true,
                panel_url: Some("loom://settings/tea".into()),
                reason: None,
            })),
        });

        assert_eq!(ownership.source, ConfigurationSource::LoomManaged);
        assert_eq!(ownership.configuration.owner, ConfigurationOwner::Loom);
        assert_eq!(
            ownership.configuration.loom_panel_url.as_deref(),
            Some("loom://settings/tea")
        );
    }

    #[test]
    fn ownership_falls_back_when_configured_loom_claim_fails() {
        let ownership = resolve_configuration_ownership(ConfigurationDiscovery {
            local_config_path: None,
            loom_base_url: Some("http://127.0.0.1:8765".into()),
            loom_claim: Some(Err("missing or invalid Loom bearer token".into())),
        });

        assert_eq!(ownership.source, ConfigurationSource::Fallback);
        assert_eq!(ownership.configuration.owner, ConfigurationOwner::Tea);
        assert_eq!(
            ownership.configuration.reason.as_deref(),
            Some("missing or invalid Loom bearer token")
        );
    }

    #[test]
    fn local_config_round_trips_as_schema_version_one() {
        let config = TeaConfiguration {
            notifications_enabled: false,
            human_ticket_default_approval_policy: "human_before_completion".into(),
            hook_ticket_default_approval_policy: "plan_only".into(),
        };

        let encoded = encode_local_config(&config).expect("encode config");
        assert!(encoded.contains("\"schema_version\":1"));
        let decoded = decode_local_config(&encoded).expect("decode config");

        assert_eq!(decoded, config);
    }
}
