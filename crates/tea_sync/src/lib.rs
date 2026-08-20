//! Tea external issue-tracker sync mapping.
//!
//! This crate is a pure, deterministic translation layer between external issue
//! trackers (starting with GitHub Issues) and Tea ticket payloads. It owns no
//! I/O: callers fetch external issues however they like (REST, webhook, export)
//! and feed the parsed JSON here, then send the resulting Tea payloads through
//! Tea's public HTTP API. Keeping this layer pure makes the mapping fully
//! unit-testable and keeps Tea API-first — sync is just another API client.
//!
//! Direction 1 (inbound): external issue -> Tea create/edit payload.
//! Direction 2 (outbound): Tea ticket status -> desired external issue state.
//!
//! Provenance: every ticket created from an external issue carries stable
//! `sync:<provider>` and `sync-id:<provider>:<external-id>` labels so a later
//! sync pass can find the Tea ticket that mirrors a given external issue and
//! avoid creating duplicates. These are treated as operator labels (they do not
//! use Tea's reserved `source:`/`policy:`/`context:` prefixes), so Tea preserves
//! its own system labels independently.

#![forbid(unsafe_code)]

use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Errors from parsing or mapping external issues.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum SyncError {
    #[error("external issue payload must be a JSON object")]
    NotAnObject,
    #[error("external issue is missing required field: {0}")]
    MissingField(&'static str),
    #[error("external issue field {name} must be a {expected}")]
    InvalidField {
        name: &'static str,
        expected: &'static str,
    },
    #[error("unsupported provider: {0}")]
    UnsupportedProvider(String),
    #[error("multiple Tea tickets mirror {provider} issue #{external_id}: {ticket_ids:?}")]
    DuplicateMirror {
        provider: String,
        external_id: String,
        ticket_ids: Vec<String>,
    },
}

/// Supported external issue-tracker providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    GitHub,
    Gitea,
}

impl Provider {
    /// Stable short slug used in provenance labels.
    pub fn slug(self) -> &'static str {
        match self {
            Provider::GitHub => "github",
            Provider::Gitea => "gitea",
        }
    }

    pub fn parse(value: &str) -> Result<Self, SyncError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "github" => Ok(Provider::GitHub),
            "gitea" => Ok(Provider::Gitea),
            other => Err(SyncError::UnsupportedProvider(other.to_string())),
        }
    }
}

/// A normalized external issue, provider-agnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalIssue {
    pub provider: Provider,
    /// Provider-native issue identifier (GitHub/Gitea: the issue `number`).
    pub external_id: String,
    pub title: String,
    pub body: String,
    /// External state, normalized to lowercase (e.g. "open", "closed").
    pub state: String,
    /// External labels (names only), used to derive priority and carried through
    /// as operator labels on the Tea ticket.
    pub labels: Vec<String>,
    /// Optional canonical URL of the external issue, recorded in the description.
    pub url: Option<String>,
}

/// The stable provenance label marking which provider a ticket was synced from.
pub fn provider_label(provider: Provider) -> String {
    format!("sync:{}", provider.slug())
}

/// The stable provenance label uniquely identifying the external issue a ticket
/// mirrors. Sync passes match on this to update rather than duplicate.
pub fn sync_id_label(provider: Provider, external_id: &str) -> String {
    format!("sync-id:{}:{}", provider.slug(), external_id.trim())
}

/// Stable, header-safe key for creating the Tea mirror of one external issue.
/// The raw external id is hashed because providers may expose Unicode or
/// whitespace that is not valid in Tea's `Idempotency-Key` contract.
pub fn create_idempotency_key(issue: &ExternalIssue) -> String {
    let provider = issue.provider.slug();
    let mut hasher = Sha256::new();
    hasher.update(provider.as_bytes());
    hasher.update([0]);
    hasher.update(issue.external_id.trim().as_bytes());
    format!("tea-sync-v1-{provider}-{:x}", hasher.finalize())
}

/// Extract the external id from a `sync-id:<provider>:<id>` label, if present and
/// matching the given provider.
pub fn external_id_from_label(provider: Provider, label: &str) -> Option<String> {
    let prefix = format!("sync-id:{}:", provider.slug());
    label.strip_prefix(&prefix).map(|id| id.to_string())
}

/// Given a ticket's labels, find the external id it was synced from (if any).
pub fn external_id_of_ticket(provider: Provider, labels: &[String]) -> Option<String> {
    labels
        .iter()
        .find_map(|label| external_id_from_label(provider, label))
}

/// Parse a raw provider issue JSON object into a normalized [`ExternalIssue`].
///
/// GitHub and Gitea issue REST payloads share the fields used here (`number`,
/// `title`, `body`, `state`, `labels[].name`, `html_url`), so one parser serves
/// both; `provider` selects the provenance slug.
pub fn parse_issue(provider: Provider, raw: &Value) -> Result<ExternalIssue, SyncError> {
    let obj = raw.as_object().ok_or(SyncError::NotAnObject)?;

    let external_id = match obj.get("number") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(_) => {
            return Err(SyncError::InvalidField {
                name: "number",
                expected: "number or non-empty string",
            })
        }
        None => return Err(SyncError::MissingField("number")),
    };

    let title = match obj.get("title") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(Value::String(_)) | None => return Err(SyncError::MissingField("title")),
        Some(_) => {
            return Err(SyncError::InvalidField {
                name: "title",
                expected: "string",
            })
        }
    };

    // Body is optional in provider APIs (can be null); treat missing/null as empty.
    let body = match obj.get("body") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(_) => {
            return Err(SyncError::InvalidField {
                name: "body",
                expected: "string",
            })
        }
    };

    let state = match obj.get("state") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_ascii_lowercase(),
        Some(Value::Null) | None => "open".to_string(),
        Some(_) => {
            return Err(SyncError::InvalidField {
                name: "state",
                expected: "string",
            })
        }
    };

    let labels = match obj.get("labels") {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                // GitHub/Gitea labels are objects with a `name` field.
                Value::Object(label) => label.get("name").and_then(Value::as_str),
                // Tolerate a plain-string label array too.
                Value::String(s) => Some(s.as_str()),
                _ => None,
            })
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(_) => {
            return Err(SyncError::InvalidField {
                name: "labels",
                expected: "array",
            })
        }
    };

    let url = match obj.get("html_url").or_else(|| obj.get("url")) {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    };

    Ok(ExternalIssue {
        provider,
        external_id,
        title,
        body,
        state,
        labels,
        url,
    })
}

/// Map an external label set to a Tea priority, if a recognized priority label is
/// present. Recognizes common conventions (`priority:high`, `p0`, `high`, etc.).
pub fn priority_from_labels(labels: &[String]) -> Option<String> {
    for label in labels {
        let l = label.to_ascii_lowercase();
        let bare = l.rsplit([':', '/']).next().unwrap_or(l.as_str()).trim();
        match bare {
            "high" | "urgent" | "critical" | "p0" | "p1" => return Some("high".to_string()),
            "low" | "minor" | "p3" | "p4" => return Some("low".to_string()),
            "normal" | "medium" | "p2" => return Some("normal".to_string()),
            _ => {}
        }
    }
    None
}

/// Operator labels to attach to a Tea ticket mirroring this external issue:
/// the provenance labels plus the external issue's own labels (namespaced under
/// `ext:` so they never collide with Tea's reserved prefixes or operator labels).
pub fn operator_labels_for(issue: &ExternalIssue) -> Vec<String> {
    let mut labels = vec![
        provider_label(issue.provider),
        sync_id_label(issue.provider, &issue.external_id),
    ];
    for label in &issue.labels {
        labels.push(format!("ext:{}", label));
    }
    labels
}

/// Compose the Tea ticket description from an external issue, appending a
/// provenance footer so a human reading the Tea ticket can trace the origin.
fn description_for(issue: &ExternalIssue) -> String {
    let mut body = issue.body.trim().to_string();
    if body.is_empty() {
        body = format!(
            "(No description provided in {} issue.)",
            issue.provider.slug()
        );
    }
    let origin = match &issue.url {
        Some(url) => format!(
            "{} issue #{} ({})",
            issue.provider.slug(),
            issue.external_id,
            url
        ),
        None => format!("{} issue #{}", issue.provider.slug(), issue.external_id),
    };
    format!("{body}\n\n---\nSynced from {origin}.")
}

/// Build a Tea `POST /v1/tickets` request body for an external issue.
///
/// Tea requires a title of >= 3 chars and description >= 10 chars; the
/// description footer guarantees the length minimum even for empty issue bodies.
pub fn to_create_request(issue: &ExternalIssue) -> Value {
    let mut body = json!({
        "title": issue.title,
        "description": description_for(issue),
        "labels": operator_labels_for(issue),
    });
    if let Some(priority) = priority_from_labels(&issue.labels) {
        body["priority"] = Value::String(priority);
    }
    body
}

/// Build a Tea `PATCH /v1/tickets/{id}` request body to update an existing ticket
/// from the current external issue. Only sends fields sync owns (title,
/// description, operator labels, priority); Tea preserves its system labels.
pub fn to_edit_request(issue: &ExternalIssue) -> Value {
    let mut body = json!({
        "title": issue.title,
        "description": description_for(issue),
        "labels": operator_labels_for(issue),
    });
    if let Some(priority) = priority_from_labels(&issue.labels) {
        body["priority"] = Value::String(priority);
    }
    body
}

/// The action a sync pass should take for one external issue, given whether a
/// mirroring Tea ticket already exists.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncAction {
    /// No mirror exists; create a new Tea ticket with this request body.
    Create(Value),
    /// A mirror exists at `ticket_id`; patch it with this request body.
    Update { ticket_id: String, body: Value },
}

/// Index of existing Tea mirrors keyed by external id: `external_id ->` the
/// sorted, deduplicated set of ticket ids carrying that `sync-id` label.
pub type MirrorIndex = HashMap<String, BTreeSet<String>>;

/// Build the mirror index for one provider in a single pass over the existing
/// Tea tickets. Sync passes plan every issue against this index instead of
/// rescanning every ticket's labels per issue.
pub fn build_mirror_index(
    provider: Provider,
    existing_tickets: &[(String, Vec<String>)],
) -> MirrorIndex {
    let prefix = format!("sync-id:{}:", provider.slug());
    let mut index = MirrorIndex::new();
    for (ticket_id, labels) in existing_tickets {
        for label in labels {
            if let Some(external_id) = label.strip_prefix(&prefix) {
                index
                    .entry(external_id.to_string())
                    .or_default()
                    .insert(ticket_id.clone());
            }
        }
    }
    index
}

/// Decide the sync action for an external issue given the set of existing Tea
/// tickets (as `(ticket_id, labels)` pairs). Matches on the `sync-id` provenance
/// label so re-syncing updates rather than duplicates.
pub fn plan_action(
    issue: &ExternalIssue,
    existing_tickets: &[(String, Vec<String>)],
) -> Result<SyncAction, SyncError> {
    let index = build_mirror_index(issue.provider, existing_tickets);
    plan_action_indexed(issue, &index)
}

/// [`plan_action`] against a prebuilt [`MirrorIndex`] (which must have been
/// built for the same provider as the issue).
pub fn plan_action_indexed(
    issue: &ExternalIssue,
    index: &MirrorIndex,
) -> Result<SyncAction, SyncError> {
    let ticket_ids = index
        .get(issue.external_id.trim())
        .map(|ticket_ids| ticket_ids.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    match ticket_ids.as_slice() {
        [] => Ok(SyncAction::Create(to_create_request(issue))),
        [ticket_id] => Ok(SyncAction::Update {
            ticket_id: ticket_id.clone(),
            body: to_edit_request(issue),
        }),
        _ => Err(SyncError::DuplicateMirror {
            provider: issue.provider.slug().to_string(),
            external_id: issue.external_id.clone(),
            ticket_ids,
        }),
    }
}

/// When an external issue is closed, the mirroring Tea ticket should move toward
/// a terminal state. This maps the external state to the Tea lifecycle action a
/// sync pass should invoke (`cancel` for closed issues), or `None` when no
/// lifecycle change is warranted.
pub fn lifecycle_action_for_state(external_state: &str) -> Option<&'static str> {
    match external_state.trim().to_ascii_lowercase().as_str() {
        "closed" => Some("cancel"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github_issue() -> Value {
        json!({
            "number": 42,
            "title": "Fix the login redirect loop",
            "body": "Users get stuck redirecting after SSO.",
            "state": "open",
            "html_url": "https://github.com/acme/app/issues/42",
            "labels": [
                { "name": "bug" },
                { "name": "priority:high" }
            ]
        })
    }

    #[test]
    fn provider_parse_and_slug() {
        assert_eq!(Provider::parse("GitHub").unwrap(), Provider::GitHub);
        assert_eq!(Provider::parse("gitea").unwrap(), Provider::Gitea);
        assert_eq!(Provider::GitHub.slug(), "github");
        assert!(matches!(
            Provider::parse("jira"),
            Err(SyncError::UnsupportedProvider(_))
        ));
    }

    #[test]
    fn parse_issue_normalizes_fields() {
        let issue = parse_issue(Provider::GitHub, &github_issue()).unwrap();
        assert_eq!(issue.external_id, "42");
        assert_eq!(issue.title, "Fix the login redirect loop");
        assert_eq!(issue.state, "open");
        assert_eq!(issue.labels, vec!["bug", "priority:high"]);
        assert_eq!(
            issue.url.as_deref(),
            Some("https://github.com/acme/app/issues/42")
        );
    }

    #[test]
    fn parse_issue_tolerates_missing_body_and_labels() {
        let raw = json!({ "number": 7, "title": "Bare issue", "state": "open" });
        let issue = parse_issue(Provider::GitHub, &raw).unwrap();
        assert_eq!(issue.body, "");
        assert!(issue.labels.is_empty());
        assert_eq!(issue.state, "open");
        assert!(issue.url.is_none());
    }

    #[test]
    fn parse_issue_requires_number_and_title() {
        let no_number = json!({ "title": "x", "state": "open" });
        assert_eq!(
            parse_issue(Provider::GitHub, &no_number),
            Err(SyncError::MissingField("number"))
        );
        let no_title = json!({ "number": 1, "state": "open" });
        assert_eq!(
            parse_issue(Provider::GitHub, &no_title),
            Err(SyncError::MissingField("title"))
        );
        assert_eq!(
            parse_issue(Provider::GitHub, &json!([])),
            Err(SyncError::NotAnObject)
        );
    }

    #[test]
    fn parse_issue_accepts_string_number_and_plain_string_labels() {
        let raw = json!({
            "number": "128",
            "title": "String id issue",
            "state": "OPEN",
            "labels": ["frontend", "  ", "p2"]
        });
        let issue = parse_issue(Provider::Gitea, &raw).unwrap();
        assert_eq!(issue.external_id, "128");
        assert_eq!(issue.state, "open");
        // blank label filtered out
        assert_eq!(issue.labels, vec!["frontend", "p2"]);
    }

    #[test]
    fn priority_mapping_recognizes_conventions() {
        assert_eq!(
            priority_from_labels(&["priority:high".to_string()]),
            Some("high".to_string())
        );
        assert_eq!(
            priority_from_labels(&["p0".to_string()]),
            Some("high".to_string())
        );
        assert_eq!(
            priority_from_labels(&["kind/low".to_string()]),
            Some("low".to_string())
        );
        assert_eq!(priority_from_labels(&["bug".to_string()]), None);
    }

    #[test]
    fn provenance_labels_are_stable_and_parseable() {
        assert_eq!(provider_label(Provider::GitHub), "sync:github");
        assert_eq!(sync_id_label(Provider::GitHub, "42"), "sync-id:github:42");
        assert_eq!(
            external_id_from_label(Provider::GitHub, "sync-id:github:42"),
            Some("42".to_string())
        );
        // wrong provider does not match
        assert_eq!(
            external_id_from_label(Provider::Gitea, "sync-id:github:42"),
            None
        );
    }

    #[test]
    fn create_idempotency_keys_are_stable_distinct_and_header_safe() {
        let mut issue = parse_issue(Provider::GitHub, &github_issue()).unwrap();
        issue.external_id = " 工单:四十二 ".to_string();
        let first = create_idempotency_key(&issue);
        let replay = create_idempotency_key(&issue);
        assert_eq!(first, replay);
        assert!(first.starts_with("tea-sync-v1-github-"));
        assert!(first.len() <= 255);
        assert!(first.bytes().all(|byte| (0x21..=0x7e).contains(&byte)));

        let mut changed_id = issue.clone();
        changed_id.external_id = "工单:四十三".to_string();
        assert_ne!(first, create_idempotency_key(&changed_id));
        let mut changed_provider = issue;
        changed_provider.provider = Provider::Gitea;
        assert_ne!(first, create_idempotency_key(&changed_provider));
    }

    #[test]
    fn operator_labels_include_provenance_and_namespaced_external_labels() {
        let issue = parse_issue(Provider::GitHub, &github_issue()).unwrap();
        let labels = operator_labels_for(&issue);
        assert!(labels.contains(&"sync:github".to_string()));
        assert!(labels.contains(&"sync-id:github:42".to_string()));
        assert!(labels.contains(&"ext:bug".to_string()));
        assert!(labels.contains(&"ext:priority:high".to_string()));
        // never emits Tea reserved prefixes
        assert!(!labels.iter().any(|l| l.starts_with("source:")
            || l.starts_with("policy:")
            || l.starts_with("context:")));
    }

    #[test]
    fn create_request_has_valid_title_description_labels_priority() {
        let issue = parse_issue(Provider::GitHub, &github_issue()).unwrap();
        let body = to_create_request(&issue);
        assert_eq!(body["title"], "Fix the login redirect loop");
        assert_eq!(body["priority"], "high");
        let desc = body["description"].as_str().unwrap();
        assert!(desc.contains("Users get stuck"));
        assert!(desc.contains("Synced from github issue #42"));
        assert!(desc.len() >= 10);
        let labels: Vec<String> = body["labels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert!(labels.contains(&"sync-id:github:42".to_string()));
    }

    #[test]
    fn create_request_synthesizes_description_for_empty_body() {
        let raw = json!({ "number": 9, "title": "Empty body", "state": "open" });
        let issue = parse_issue(Provider::GitHub, &raw).unwrap();
        let body = to_create_request(&issue);
        let desc = body["description"].as_str().unwrap();
        assert!(desc.len() >= 10);
        assert!(desc.contains("No description provided"));
        // no priority label present -> field omitted
        assert!(body.get("priority").is_none());
    }

    #[test]
    fn plan_action_creates_when_no_mirror_exists() {
        let issue = parse_issue(Provider::GitHub, &github_issue()).unwrap();
        let action = plan_action(&issue, &[]).unwrap();
        match action {
            SyncAction::Create(body) => assert_eq!(body["title"], "Fix the login redirect loop"),
            other => panic!("expected Create, got {other:?}"),
        }
    }

    #[test]
    fn plan_action_updates_when_mirror_exists() {
        let issue = parse_issue(Provider::GitHub, &github_issue()).unwrap();
        let existing = vec![
            ("unrelated".to_string(), vec!["source:human".to_string()]),
            (
                "ticket-1".to_string(),
                vec!["sync:github".to_string(), "sync-id:github:42".to_string()],
            ),
        ];
        match plan_action(&issue, &existing).unwrap() {
            SyncAction::Update { ticket_id, body } => {
                assert_eq!(ticket_id, "ticket-1");
                assert_eq!(body["title"], "Fix the login redirect loop");
            }
            other => panic!("expected Update, got {other:?}"),
        }
    }

    #[test]
    fn mirror_index_groups_tickets_by_external_id_in_one_pass() {
        let existing = vec![
            ("unrelated".to_string(), vec!["source:human".to_string()]),
            (
                "ticket-1".to_string(),
                vec!["sync:github".to_string(), "sync-id:github:42".to_string()],
            ),
            ("ticket-2".to_string(), vec!["sync-id:github:7".to_string()]),
            (
                "ticket-3".to_string(),
                vec!["sync-id:github:42".to_string()],
            ),
        ];

        let index = build_mirror_index(Provider::GitHub, &existing);

        assert_eq!(index.len(), 2);
        assert_eq!(
            index["42"].iter().cloned().collect::<Vec<_>>(),
            vec!["ticket-1".to_string(), "ticket-3".to_string()]
        );
        assert_eq!(
            index["7"].iter().cloned().collect::<Vec<_>>(),
            vec!["ticket-2".to_string()]
        );
    }

    #[test]
    fn mirror_index_ignores_other_providers_and_dedups_ticket_ids() {
        let existing = vec![
            (
                "gitea-only".to_string(),
                vec!["sync-id:gitea:42".to_string()],
            ),
            (
                "ticket-b".to_string(),
                vec!["sync-id:github:42".to_string()],
            ),
            (
                "ticket-b".to_string(),
                vec!["sync-id:github:42".to_string()],
            ),
        ];

        let index = build_mirror_index(Provider::GitHub, &existing);

        assert_eq!(index.len(), 1);
        assert_eq!(
            index["42"].iter().cloned().collect::<Vec<_>>(),
            vec!["ticket-b".to_string()]
        );
        assert!(build_mirror_index(Provider::Gitea, &existing).contains_key("42"));
    }

    #[test]
    fn plan_action_indexed_matches_unindexed_planning() {
        let issue = parse_issue(Provider::GitHub, &github_issue()).unwrap();
        let existing = vec![(
            "ticket-1".to_string(),
            vec!["sync-id:github:42".to_string()],
        )];
        let index = build_mirror_index(Provider::GitHub, &existing);

        assert_eq!(
            plan_action_indexed(&issue, &index),
            plan_action(&issue, &existing)
        );
        assert_eq!(
            plan_action_indexed(&issue, &MirrorIndex::new()),
            plan_action(&issue, &[])
        );
    }

    #[test]
    fn plan_action_rejects_ambiguous_mirrors() {
        let issue = parse_issue(Provider::GitHub, &github_issue()).unwrap();
        let matching_labels = vec!["sync-id:github:42".to_string()];
        let existing = vec![
            ("ticket-b".to_string(), matching_labels.clone()),
            ("ticket-a".to_string(), matching_labels.clone()),
            ("ticket-a".to_string(), matching_labels),
        ];

        assert_eq!(
            plan_action(&issue, &existing),
            Err(SyncError::DuplicateMirror {
                provider: "github".to_string(),
                external_id: "42".to_string(),
                ticket_ids: vec!["ticket-a".to_string(), "ticket-b".to_string()],
            })
        );
    }

    #[test]
    fn external_id_of_ticket_finds_provenance() {
        let labels = vec!["source:human".to_string(), "sync-id:github:42".to_string()];
        assert_eq!(
            external_id_of_ticket(Provider::GitHub, &labels),
            Some("42".to_string())
        );
        assert_eq!(external_id_of_ticket(Provider::Gitea, &labels), None);
    }

    #[test]
    fn closed_issue_maps_to_cancel_lifecycle() {
        assert_eq!(lifecycle_action_for_state("closed"), Some("cancel"));
        assert_eq!(lifecycle_action_for_state("CLOSED"), Some("cancel"));
        assert_eq!(lifecycle_action_for_state("open"), None);
    }
}
