//! Tea external issue-tracker sync CLI.
//!
//! One-shot sync pass: fetch issues from an external tracker (GitHub or Gitea),
//! list existing Tea tickets, and for each issue create or update the mirroring
//! Tea ticket via Tea's public HTTP API. All field translation and dedup logic
//! lives in the pure `tea_sync` crate; this binary owns only I/O (REST fetch,
//! Tea API calls) and orchestration.
//!
//! Safe by default: runs in dry-run mode unless `--apply` is passed, so an
//! operator can preview the planned create/update/close actions first.

#![forbid(unsafe_code)]

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use serde_json::Value;
use tea_sync::{
    external_id_of_ticket, lifecycle_action_for_state, parse_issue, plan_action, Provider,
    SyncAction,
};

#[derive(Debug, Parser)]
#[command(
    name = "tea-sync",
    about = "Sync external tracker issues into Tea tickets"
)]
struct Cli {
    /// External provider: github or gitea.
    #[arg(long, default_value = "github")]
    provider: String,
    /// Owner/org of the repository, e.g. "aiaimimi0920".
    #[arg(long)]
    owner: String,
    /// Repository name, e.g. "Neuro".
    #[arg(long)]
    repo: String,
    /// Base URL of the tracker REST API. Defaults to GitHub's public API.
    #[arg(
        long,
        env = "TEA_SYNC_API_BASE",
        default_value = "https://api.github.com"
    )]
    api_base: String,
    /// Optional bearer token for the tracker API (avoids rate limits / private repos).
    #[arg(long, env = "TEA_SYNC_TOKEN")]
    token: Option<String>,
    /// Base URL of the Tea daemon HTTP API.
    #[arg(long, env = "TEA_SERVER_URL", default_value = "http://127.0.0.1:48910")]
    tea_url: String,
    /// Bearer token for the Tea daemon HTTP API.
    #[arg(long, env = "TEA_AUTH_TOKEN", default_value = "dev-token")]
    tea_token: String,
    /// Apply changes. Without this flag the pass is a dry run (preview only).
    #[arg(long)]
    apply: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let provider =
        Provider::parse(&cli.provider).map_err(|err| anyhow!("invalid --provider: {err}"))?;

    let http = reqwest::Client::builder()
        .user_agent("tea-sync/0.1")
        .build()
        .context("build HTTP client")?;

    let tracker = TrackerClient {
        api_base: cli.api_base.trim_end_matches('/').to_string(),
        token: cli.token.clone(),
        http: http.clone(),
    };
    let tea = TeaClient {
        base_url: cli.tea_url.trim_end_matches('/').to_string(),
        token: cli.tea_token.clone(),
        http,
    };

    // 1. Fetch open+closed issues from the tracker.
    let issues = tracker
        .fetch_issues(provider, &cli.owner, &cli.repo)
        .await
        .context("fetch tracker issues")?;

    // 2. Snapshot existing Tea tickets as (id, labels) so we can dedup on the
    //    sync-id provenance label.
    let tickets = tea.list_tickets().await.context("list Tea tickets")?;
    let existing: Vec<(String, Vec<String>)> = tickets
        .iter()
        .filter_map(|t| {
            let id = t.get("id").and_then(Value::as_str)?.to_string();
            let labels = t
                .get("labels")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Some((id, labels))
        })
        .collect();

    let mut created = 0usize;
    let mut updated = 0usize;
    let mut closed = 0usize;

    for raw in &issues {
        let issue = match parse_issue(provider, raw) {
            Ok(issue) => issue,
            Err(err) => {
                eprintln!("skip issue: {err}");
                continue;
            }
        };

        match plan_action(&issue, &existing) {
            SyncAction::Create(body) => {
                println!(
                    "CREATE  {} #{}  \"{}\"",
                    issue.provider.slug(),
                    issue.external_id,
                    issue.title
                );
                if cli.apply {
                    tea.create_ticket(&body).await.with_context(|| {
                        format!("create ticket for issue #{}", issue.external_id)
                    })?;
                    created += 1;
                }
            }
            SyncAction::Update { ticket_id, body } => {
                println!(
                    "UPDATE  {} #{}  -> ticket {}",
                    issue.provider.slug(),
                    issue.external_id,
                    ticket_id
                );
                if cli.apply {
                    tea.edit_ticket(&ticket_id, &body)
                        .await
                        .with_context(|| format!("edit ticket {ticket_id}"))?;
                    updated += 1;
                }
                // 3. If the external issue is closed, move the mirror toward a
                //    terminal state via the mapped lifecycle action.
                if let Some(action) = lifecycle_action_for_state(&issue.state) {
                    println!("  state={} -> tea {}", issue.state, action);
                    if cli.apply {
                        // Best-effort: a ticket already terminal returns 409; ignore.
                        if tea.lifecycle(&ticket_id, action).await.is_ok() {
                            closed += 1;
                        }
                    }
                }
            }
        }
    }

    // Report tickets that were synced but whose issue vanished from the tracker
    // (informational only; sync never deletes Tea tickets).
    let fetched_ids: Vec<String> = issues
        .iter()
        .filter_map(|raw| parse_issue(provider, raw).ok().map(|i| i.external_id))
        .collect();
    for (id, labels) in &existing {
        if let Some(ext) = external_id_of_ticket(provider, labels) {
            if !fetched_ids.contains(&ext) {
                println!(
                    "ORPHAN  ticket {id} mirrors {} #{ext} (not in fetch)",
                    provider.slug()
                );
            }
        }
    }

    let mode = if cli.apply {
        "applied"
    } else {
        "dry-run (use --apply to write)"
    };
    println!(
        "\ntea-sync {mode}: {} issues, {created} created, {updated} updated, {closed} closed",
        issues.len()
    );
    Ok(())
}

/// Minimal external tracker REST client (GitHub / Gitea compatible issue list).
struct TrackerClient {
    api_base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl TrackerClient {
    async fn fetch_issues(
        &self,
        provider: Provider,
        owner: &str,
        repo: &str,
    ) -> Result<Vec<Value>> {
        // Both GitHub and Gitea expose /repos/{owner}/{repo}/issues; Gitea nests
        // under /api/v1 which the caller includes in --api-base.
        let path = match provider {
            Provider::GitHub => format!("/repos/{owner}/{repo}/issues?state=all&per_page=100"),
            Provider::Gitea => format!("/repos/{owner}/{repo}/issues?state=all&limit=100"),
        };
        let url = format!("{}{}", self.api_base, path);
        let mut req = self.http.get(&url).header("accept", "application/json");
        if let Some(token) = &self.token {
            req = req.bearer_auth(token);
        }
        let resp = req.send().await.with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!("tracker API {status}: {text}"));
        }
        let value: Value = serde_json::from_str(&text).context("parse tracker JSON")?;
        // GitHub returns a bare array; some Gitea deployments wrap in {data:[]}.
        let arr = match value {
            Value::Array(a) => a,
            Value::Object(mut o) => o
                .remove("data")
                .and_then(|d| d.as_array().cloned())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        // GitHub's issues endpoint also returns pull requests; skip those.
        Ok(arr
            .into_iter()
            .filter(|i| i.get("pull_request").is_none())
            .collect())
    }
}

/// Minimal Tea HTTP API client (bearer auth, JSON).
struct TeaClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl TeaClient {
    async fn list_tickets(&self) -> Result<Vec<Value>> {
        let url = format!("{}/v1/tickets", self.base_url);
        let resp = self.http.get(&url).bearer_auth(&self.token).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(anyhow!("Tea list tickets {status}: {text}"));
        }
        let value: Value = serde_json::from_str(&text)?;
        // Tolerate either a bare array or a {tickets:[]} envelope.
        Ok(match value {
            Value::Array(a) => a,
            Value::Object(mut o) => o
                .remove("tickets")
                .and_then(|t| t.as_array().cloned())
                .unwrap_or_default(),
            _ => Vec::new(),
        })
    }

    async fn create_ticket(&self, body: &Value) -> Result<()> {
        let url = format!("{}/v1/tickets", self.base_url);
        self.send(self.http.post(&url).json(body)).await
    }

    async fn edit_ticket(&self, ticket_id: &str, body: &Value) -> Result<()> {
        let url = format!("{}/v1/tickets/{}", self.base_url, urlencode(ticket_id));
        self.send(self.http.patch(&url).json(body)).await
    }

    async fn lifecycle(&self, ticket_id: &str, action: &str) -> Result<()> {
        let url = format!(
            "{}/v1/tickets/{}/{}",
            self.base_url,
            urlencode(ticket_id),
            action
        );
        self.send(self.http.post(&url)).await
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<()> {
        let resp = req.bearer_auth(&self.token).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Tea API {status}: {text}"));
        }
        Ok(())
    }
}

/// Percent-encode a path segment (ticket ids are UUIDs, but be safe).
fn urlencode(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
            _ => format!("%{:02X}", c as u32),
        })
        .collect()
}
