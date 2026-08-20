#![forbid(unsafe_code)]

use std::fs::{OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const LOCAL_CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCAL_CONFIG_LOCK_RETRY: Duration = Duration::from_millis(10);

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
    #[error("invalid Tea configuration update: {0}")]
    InvalidUpdate(String),
    #[error("unsupported Tea local config schema version {found}; this binary supports version {supported}")]
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    #[error("Tea local config I/O: {0}")]
    Io(#[from] std::io::Error),
}

pub fn resolve_configuration_ownership(
    discovery: ConfigurationDiscovery,
) -> ConfigurationOwnership {
    let local_config_path = discovery.local_config_path;
    let loom_base_url = discovery.loom_base_url;

    match discovery.loom_claim {
        Some(Ok(claim)) => {
            let app_matches = claim.app == "tea";
            let panel_url = if app_matches {
                claim
                    .panel_url
                    .filter(|value| is_safe_loom_panel_url(value))
            } else {
                None
            };
            let loom_managed = app_matches && claim.managed && panel_url.is_some();
            ConfigurationOwnership {
                source: if !app_matches {
                    ConfigurationSource::Fallback
                } else if loom_managed {
                    ConfigurationSource::LoomManaged
                } else {
                    ConfigurationSource::Local
                },
                configuration: ConfigurationDetails {
                    owner: if loom_managed {
                        ConfigurationOwner::Loom
                    } else {
                        ConfigurationOwner::Tea
                    },
                    local_config_path,
                    loom_base_url,
                    loom_panel_url: panel_url,
                    reason: if app_matches {
                        claim.reason
                    } else {
                        Some("Loom configuration claim did not identify app tea".to_string())
                    },
                },
            }
        }
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
    if document.schema_version != 1 {
        return Err(ConfigError::UnsupportedSchemaVersion {
            found: document.schema_version,
            supported: 1,
        });
    }
    Ok(document.config)
}

pub fn read_local_config_file(path: &Path) -> Result<Option<TeaConfiguration>, ConfigError> {
    with_local_config_lock(path, || read_local_config_file_locked(path))
}

fn read_local_config_file_locked(path: &Path) -> Result<Option<TeaConfiguration>, ConfigError> {
    if path.exists() {
        let primary = std::fs::read_to_string(path)
            .map_err(ConfigError::Io)
            .and_then(|value| decode_local_config(&value));
        match primary {
            Ok(config) => return Ok(Some(config)),
            Err(error) if !config_error_allows_backup_recovery(&error) => return Err(error),
            Err(primary_error) => {
                let backup = config_backup_path(path);
                if !backup.is_file() {
                    return Err(primary_error);
                }
                let backup_config = match std::fs::read_to_string(&backup)
                    .map_err(ConfigError::Io)
                    .and_then(|value| decode_local_config(&value))
                {
                    Ok(config) => config,
                    Err(_) => return Err(primary_error),
                };
                restore_local_config_backup(path, &backup)?;
                return Ok(Some(backup_config));
            }
        }
    }
    let backup = config_backup_path(path);
    if !backup.exists() {
        return Ok(None);
    }
    let config = decode_local_config(&std::fs::read_to_string(&backup)?)?;
    std::fs::rename(&backup, path)?;
    Ok(Some(config))
}

fn config_error_allows_backup_recovery(error: &ConfigError) -> bool {
    matches!(error, ConfigError::Encode(_))
        || matches!(error, ConfigError::Io(error) if error.kind() == ErrorKind::InvalidData)
}

fn restore_local_config_backup(path: &Path, backup: &Path) -> Result<(), ConfigError> {
    let displaced = config_temporary_path(path);
    std::fs::rename(path, &displaced)?;
    if let Err(error) = std::fs::rename(backup, path) {
        let _ = std::fs::rename(&displaced, path);
        return Err(ConfigError::Io(error));
    }
    let _ = std::fs::remove_file(displaced);
    Ok(())
}

pub fn write_local_config_atomic(
    path: &Path,
    config: &TeaConfiguration,
) -> Result<(), ConfigError> {
    let encoded = encode_local_config(config)?;
    with_local_config_lock(path, || write_local_config_atomic_locked(path, &encoded))
}

pub fn update_local_config_atomic(
    path: &Path,
    fallback: TeaConfiguration,
    update: impl FnOnce(TeaConfiguration) -> Result<TeaConfiguration, ConfigError>,
) -> Result<TeaConfiguration, ConfigError> {
    with_local_config_lock(path, || {
        let current = read_local_config_file_locked(path)?.unwrap_or(fallback);
        let updated = update(current)?;
        let encoded = encode_local_config(&updated)?;
        write_local_config_atomic_locked(path, &encoded)?;
        Ok(updated)
    })
}

fn write_local_config_atomic_locked(path: &Path, encoded: &str) -> Result<(), ConfigError> {
    let temporary = config_temporary_path(path);
    let backup = config_backup_path(path);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    if let Err(error) = (|| -> Result<(), std::io::Error> {
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
        drop(file);

        if !path.exists() {
            return std::fs::rename(&temporary, path);
        }
        if backup.exists() {
            std::fs::remove_file(&backup)?;
        }
        std::fs::rename(path, &backup)?;
        if let Err(error) = std::fs::rename(&temporary, path) {
            let _ = std::fs::rename(&backup, path);
            return Err(error);
        }
        let _ = std::fs::remove_file(&backup);
        Ok(())
    })() {
        let _ = std::fs::remove_file(&temporary);
        return Err(ConfigError::Io(error));
    }
    Ok(())
}

fn with_local_config_lock<T>(
    path: &Path,
    operation: impl FnOnce() -> Result<T, ConfigError>,
) -> Result<T, ConfigError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let lock_path = config_lock_path(path);
    // Keep this file in place permanently. Deleting and recreating a lock file can
    // let two processes lock different file objects for the same config path.
    let lock_file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    let started = Instant::now();
    loop {
        match lock_file.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if started.elapsed() < LOCAL_CONFIG_LOCK_TIMEOUT => {
                std::thread::sleep(LOCAL_CONFIG_LOCK_RETRY);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(ConfigError::Io(std::io::Error::new(
                    ErrorKind::TimedOut,
                    format!(
                        "timed out acquiring Tea local config lock {}",
                        lock_path.display()
                    ),
                )));
            }
            Err(TryLockError::Error(error)) => return Err(ConfigError::Io(error)),
        }
    }
    operation()
}

fn config_backup_path(path: &Path) -> PathBuf {
    let mut file_name = path.file_name().unwrap_or_default().to_os_string();
    file_name.push(".bak");
    path.with_file_name(file_name)
}

fn config_lock_path(path: &Path) -> PathBuf {
    let mut file_name = path.file_name().unwrap_or_default().to_os_string();
    file_name.push(".lock");
    path.with_file_name(file_name)
}

fn config_temporary_path(path: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let mut file_name = path.file_name().unwrap_or_default().to_os_string();
    file_name.push(format!(".{}.{}.tmp", std::process::id(), nanos));
    path.with_file_name(file_name)
}

pub fn is_safe_loom_panel_url(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed != value {
        return false;
    }
    let Ok(parsed) = url::Url::parse(trimmed) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https" | "loom")
        && parsed.host_str().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_config_rejects_unknown_schema_versions() {
        let value = serde_json::json!({
            "schema_version": 2,
            "notifications_enabled": true,
            "human_ticket_default_approval_policy": "human_before_execute",
            "hook_ticket_default_approval_policy": "plan_only"
        });

        assert!(matches!(
            decode_local_config(&value.to_string()).unwrap_err(),
            ConfigError::UnsupportedSchemaVersion {
                found: 2,
                supported: 1
            }
        ));
    }

    #[test]
    fn local_config_atomic_write_replaces_and_recovers_backup() {
        let path = std::env::temp_dir().join(format!(
            "tea-config-atomic-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = TeaConfiguration::default();
        write_local_config_atomic(&path, &first).unwrap();
        assert_eq!(read_local_config_file(&path).unwrap(), Some(first));

        let replacement = TeaConfiguration {
            notifications_enabled: false,
            human_ticket_default_approval_policy: "manual_only".to_string(),
            hook_ticket_default_approval_policy: "plan_only".to_string(),
        };
        write_local_config_atomic(&path, &replacement).unwrap();
        assert_eq!(
            read_local_config_file(&path).unwrap(),
            Some(replacement.clone())
        );

        let backup = config_backup_path(&path);
        std::fs::rename(&path, &backup).unwrap();
        assert_eq!(read_local_config_file(&path).unwrap(), Some(replacement));
        assert!(path.exists());
        assert!(!backup.exists());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(config_lock_path(&path));
    }

    #[test]
    fn local_config_recovers_corrupt_primary_from_valid_backup() {
        let path = std::env::temp_dir().join(format!(
            "tea-config-corrupt-recovery-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let backup = config_backup_path(&path);
        let expected = TeaConfiguration {
            notifications_enabled: false,
            human_ticket_default_approval_policy: "manual_only".to_string(),
            hook_ticket_default_approval_policy: "plan_only".to_string(),
        };
        std::fs::write(&path, "{\"schema_version\":1,").unwrap();
        std::fs::write(&backup, encode_local_config(&expected).unwrap()).unwrap();

        assert_eq!(
            read_local_config_file(&path).unwrap(),
            Some(expected.clone())
        );
        assert_eq!(read_local_config_file(&path).unwrap(), Some(expected));
        assert!(!backup.exists());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(config_lock_path(&path));
    }

    #[test]
    fn local_config_does_not_hide_unsupported_primary_schema_with_backup() {
        let path = std::env::temp_dir().join(format!(
            "tea-config-future-schema-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let backup = config_backup_path(&path);
        std::fs::write(
            &path,
            serde_json::json!({
                "schema_version": 2,
                "notifications_enabled": false,
                "human_ticket_default_approval_policy": "manual_only",
                "hook_ticket_default_approval_policy": "plan_only"
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            &backup,
            encode_local_config(&TeaConfiguration::default()).unwrap(),
        )
        .unwrap();

        assert!(matches!(
            read_local_config_file(&path).unwrap_err(),
            ConfigError::UnsupportedSchemaVersion { found: 2, .. }
        ));
        assert!(backup.exists());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&backup);
        let _ = std::fs::remove_file(config_lock_path(&path));
    }

    #[test]
    fn local_config_concurrent_writers_serialize_backup_swap() {
        let path = std::env::temp_dir().join(format!(
            "tea-config-concurrent-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        write_local_config_atomic(&path, &TeaConfiguration::default()).unwrap();

        let configurations = (0..8)
            .map(|index| TeaConfiguration {
                notifications_enabled: index % 2 == 0,
                human_ticket_default_approval_policy: format!("concurrent-human-{index}"),
                hook_ticket_default_approval_policy: format!("concurrent-hook-{index}"),
            })
            .collect::<Vec<_>>();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(configurations.len() + 1));
        let handles = configurations
            .iter()
            .cloned()
            .map(|configuration| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..4 {
                        write_local_config_atomic(&path, &configuration).unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        for handle in handles {
            handle.join().unwrap();
        }

        let final_configuration = read_local_config_file(&path).unwrap().unwrap();
        assert!(configurations.contains(&final_configuration));
        assert!(!config_backup_path(&path).exists());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(config_lock_path(&path));
    }

    #[test]
    fn local_config_concurrent_field_updates_do_not_lose_changes() {
        let path = std::env::temp_dir().join(format!(
            "tea-config-concurrent-update-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        write_local_config_atomic(&path, &TeaConfiguration::default()).unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let notifications = {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                update_local_config_atomic(&path, TeaConfiguration::default(), |mut config| {
                    config.notifications_enabled = false;
                    Ok(config)
                })
                .unwrap();
            })
        };
        let policy = {
            let path = path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                update_local_config_atomic(&path, TeaConfiguration::default(), |mut config| {
                    config.human_ticket_default_approval_policy = "manual_only".to_string();
                    Ok(config)
                })
                .unwrap();
            })
        };
        barrier.wait();
        notifications.join().unwrap();
        policy.join().unwrap();

        let final_configuration = read_local_config_file(&path).unwrap().unwrap();
        assert!(!final_configuration.notifications_enabled);
        assert_eq!(
            final_configuration.human_ticket_default_approval_policy,
            "manual_only"
        );
        assert_eq!(
            final_configuration.hook_ticket_default_approval_policy,
            "plan_only"
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(config_lock_path(&path));
    }

    #[test]
    fn local_config_missing_file_update_uses_last_known_fallback() {
        let path = std::env::temp_dir().join(format!(
            "tea-config-missing-update-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let fallback = TeaConfiguration {
            notifications_enabled: false,
            human_ticket_default_approval_policy: "manual_only".to_string(),
            hook_ticket_default_approval_policy: "plan_only".to_string(),
        };

        let updated = update_local_config_atomic(&path, fallback, |mut config| {
            config.hook_ticket_default_approval_policy = "human_before_execute".to_string();
            Ok(config)
        })
        .unwrap();

        assert!(!updated.notifications_enabled);
        assert_eq!(updated.human_ticket_default_approval_policy, "manual_only");
        assert_eq!(
            updated.hook_ticket_default_approval_policy,
            "human_before_execute"
        );
        assert_eq!(read_local_config_file(&path).unwrap(), Some(updated));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(config_lock_path(&path));
    }

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
    fn ownership_rejects_unsafe_loom_panel_url_schemes() {
        for panel_url in [
            "javascript:alert(document.domain)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            " https://loom.example/settings/tea",
            "https://user@loom.example/settings/tea",
        ] {
            let ownership = resolve_configuration_ownership(ConfigurationDiscovery {
                local_config_path: Some("C:\\tea\\config.json".into()),
                loom_base_url: Some("http://127.0.0.1:8765".into()),
                loom_claim: Some(Ok(LoomConfigurationClaim {
                    app: "tea".into(),
                    managed: true,
                    panel_url: Some(panel_url.into()),
                    reason: None,
                })),
            });

            assert_eq!(ownership.source, ConfigurationSource::Local);
            assert_eq!(ownership.configuration.owner, ConfigurationOwner::Tea);
            assert_eq!(ownership.configuration.loom_panel_url, None);
        }
    }

    #[test]
    fn ownership_rejects_claims_for_another_app() {
        let ownership = resolve_configuration_ownership(ConfigurationDiscovery {
            local_config_path: Some("C:\\tea\\config.json".into()),
            loom_base_url: Some("http://127.0.0.1:8765".into()),
            loom_claim: Some(Ok(LoomConfigurationClaim {
                app: "other-app".into(),
                managed: true,
                panel_url: Some("https://loom.example/settings/other-app".into()),
                reason: None,
            })),
        });

        assert_eq!(ownership.source, ConfigurationSource::Fallback);
        assert_eq!(ownership.configuration.owner, ConfigurationOwner::Tea);
        assert_eq!(ownership.configuration.loom_panel_url, None);
        assert_eq!(
            ownership.configuration.reason.as_deref(),
            Some("Loom configuration claim did not identify app tea")
        );
    }

    #[test]
    fn loom_panel_url_accepts_only_explicit_web_or_loom_links() {
        assert!(is_safe_loom_panel_url("loom://settings/tea"));
        assert!(is_safe_loom_panel_url(
            "https://loom.example/settings/apps/tea"
        ));
        assert!(is_safe_loom_panel_url("http://127.0.0.1:8765/settings/tea"));
        assert!(!is_safe_loom_panel_url("javascript:alert(1)"));
        assert!(!is_safe_loom_panel_url("//loom.example/settings/tea"));
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
