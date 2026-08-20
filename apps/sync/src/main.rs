//! Tea external issue-tracker sync CLI.
//!
//! One-shot sync pass: fetch issues from an external tracker (GitHub or Gitea),
//! list existing Tea tickets, and for each issue create or update the mirroring
//! Tea ticket via Tea's public HTTP API. Field translation and provenance matching
//! live in the pure `tea_sync` crate; this binary owns I/O and pass-level safeguards
//! such as rejecting duplicate external IDs in one tracker response.
//!
//! Safe by default: runs in dry-run mode unless `--apply` is passed, so an
//! operator can preview the planned create/update/close actions first.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, HashSet},
    time::Duration,
};

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use reqwest::StatusCode;
use serde_json::Value;
use tea_http_read::{
    error_body_preview, percent_encode_component as encode_url_component, ReadLimitedError,
};
use tea_sync::{
    build_mirror_index, create_idempotency_key, lifecycle_action_for_state, parse_issue,
    plan_action, plan_action_indexed, ExternalIssue, Provider, SyncAction,
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

#[derive(Clone, Copy)]
struct GetRetryPolicy {
    max_attempts: usize,
    base_delay: Duration,
    max_delay: Duration,
    total_timeout: Duration,
}

const GET_RETRY_POLICY: GetRetryPolicy = GetRetryPolicy {
    max_attempts: 3,
    base_delay: Duration::from_millis(100),
    max_delay: Duration::from_secs(2),
    total_timeout: Duration::from_secs(300),
};
const MAX_SYNC_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
const TEA_TICKET_PAGE_SIZE: usize = 200;
const MAX_TEA_TICKET_PAGES: usize = 1_000;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let provider =
        Provider::parse(&cli.provider).map_err(|err| anyhow!("invalid --provider: {err}"))?;

    let http = build_http_client()?;

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
    let raw_issues = tracker
        .fetch_issues(provider, &cli.owner, &cli.repo)
        .await
        .context("fetch tracker issues")?;
    let (issues, skipped_issues) = parse_unique_issues(provider, &raw_issues);
    for warning in &skipped_issues {
        eprintln!("skip issue: {warning}");
    }

    // 2. Snapshot existing Tea tickets as (id, labels) so we can dedup on the
    //    sync-id provenance label.
    let tickets = tea.list_tickets().await.context("list Tea tickets")?;
    let existing = decode_ticket_refs(&tickets).context("decode Tea ticket snapshot")?;

    let mut created = 0usize;
    let mut updated = 0usize;
    let mut closed = 0usize;

    // Preflight every plan before the first write. An ambiguous existing mirror
    // must fail the pass without leaving earlier issues partially applied.
    // The mirror index is built once so planning does not rescan every ticket's
    // labels per issue.
    let mirror_index = build_mirror_index(provider, &existing);
    let plans = issues
        .iter()
        .map(|issue| {
            plan_action_indexed(issue, &mirror_index)
                .with_context(|| {
                    format!(
                        "plan {} issue #{}",
                        issue.provider.slug(),
                        issue.external_id
                    )
                })
                .map(|action| (issue, action))
        })
        .collect::<Result<Vec<_>>>()?;

    for (issue, action) in plans {
        match action {
            SyncAction::Create(body) => {
                println!(
                    "CREATE  {} #{}  \"{}\"",
                    issue.provider.slug(),
                    issue.external_id,
                    issue.title
                );
                let created_ticket_id = if cli.apply {
                    let idempotency_key = create_idempotency_key(issue);
                    let ticket_id = match tea
                        .create_ticket(&body, &idempotency_key)
                        .await
                        .with_context(|| {
                            format!("create ticket for issue #{}", issue.external_id)
                        })? {
                        CreateTicketOutcome::Created(ticket_id) => {
                            created += 1;
                            ticket_id
                        }
                        CreateTicketOutcome::Conflict => {
                            println!("  create conflict -> refresh and reconcile mirror");
                            let ticket_id = recover_create_conflict(&tea, issue)
                                .await
                                .with_context(|| {
                                    format!(
                                        "reconcile conflicted create for issue #{}",
                                        issue.external_id
                                    )
                                })?;
                            updated += 1;
                            ticket_id
                        }
                    };
                    Some(ticket_id)
                } else {
                    None
                };
                if apply_lifecycle_for_state(&tea, issue, created_ticket_id.as_deref(), cli.apply)
                    .await?
                {
                    closed += 1;
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
                if apply_lifecycle_for_state(&tea, issue, Some(&ticket_id), cli.apply).await? {
                    closed += 1;
                }
            }
        }
    }

    // Report tickets that were synced but whose issue vanished from the tracker
    // (informational only; sync never deletes Tea tickets).
    let fetched_ids: HashSet<&str> = issues
        .iter()
        .map(|issue| issue.external_id.as_str())
        .collect();
    let mut orphans: Vec<(&String, &String)> = Vec::new();
    for (ext, ticket_ids) in &mirror_index {
        if fetched_ids.contains(ext.as_str()) {
            continue;
        }
        for id in ticket_ids {
            orphans.push((id, ext));
        }
    }
    orphans.sort_unstable();
    for (id, ext) in orphans {
        println!(
            "ORPHAN  ticket {id} mirrors {} #{ext} (not in fetch)",
            provider.slug()
        );
    }

    let mode = if cli.apply {
        "applied"
    } else {
        "dry-run (use --apply to write)"
    };
    println!(
        "\ntea-sync {mode}: {} issues, {} skipped, {created} created, {updated} updated, {closed} closed",
        issues.len(),
        skipped_issues.len()
    );
    Ok(())
}

fn decode_ticket_refs(tickets: &[Value]) -> Result<Vec<(String, Vec<String>)>> {
    tickets
        .iter()
        .enumerate()
        .map(|(index, ticket)| {
            let id = ticket
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .with_context(|| format!("Tea ticket at index {index} has no non-empty id"))?;
            let labels = ticket
                .get("labels")
                .and_then(Value::as_array)
                .with_context(|| format!("Tea ticket {id} has no labels array"))?
                .iter()
                .enumerate()
                .map(|(label_index, label)| {
                    label.as_str().map(str::to_string).with_context(|| {
                        format!("Tea ticket {id} label at index {label_index} is not a string")
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            Ok((id.to_string(), labels))
        })
        .collect()
}

async fn recover_create_conflict(tea: &TeaClient, issue: &ExternalIssue) -> Result<String> {
    let tickets = tea
        .list_tickets()
        .await
        .context("refresh Tea tickets after create conflict")?;
    let existing = decode_ticket_refs(&tickets).context("decode refreshed Tea ticket snapshot")?;
    match plan_action(issue, &existing).context("plan create conflict recovery")? {
        SyncAction::Update { ticket_id, body } => {
            tea.edit_ticket(&ticket_id, &body)
                .await
                .with_context(|| format!("refresh recovered mirror ticket {ticket_id}"))?;
            Ok(ticket_id)
        }
        SyncAction::Create(_) => Err(anyhow!(
            "Tea reported an idempotency conflict but no mirror with the expected sync-id label exists"
        )),
    }
}

async fn apply_lifecycle_for_state(
    tea: &TeaClient,
    issue: &ExternalIssue,
    ticket_id: Option<&str>,
    apply: bool,
) -> Result<bool> {
    let Some(action) = lifecycle_action_for_state(&issue.state) else {
        return Ok(false);
    };
    println!("  state={} -> tea {}", issue.state, action);
    if !apply {
        return Ok(false);
    }
    let ticket_id = ticket_id.context("created Tea ticket response did not include an id")?;

    // A conflicting transition is idempotent only when a status readback proves
    // that the ticket already reached this action's target state.
    tea.lifecycle(ticket_id, action)
        .await
        .with_context(|| format!("apply lifecycle action {action} to ticket {ticket_id}"))
}

fn build_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("tea-sync/0.1")
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(300))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("build HTTP client")
}

fn parse_unique_issues(
    provider: Provider,
    raw_issues: &[Value],
) -> (Vec<ExternalIssue>, Vec<String>) {
    let mut issues = Vec::with_capacity(raw_issues.len());
    let mut seen_external_ids = HashSet::with_capacity(raw_issues.len());
    let mut warnings = Vec::new();

    for raw in raw_issues {
        match parse_issue(provider, raw) {
            Ok(issue) if seen_external_ids.insert(issue.external_id.clone()) => {
                issues.push(issue);
            }
            Ok(issue) => warnings.push(format!(
                "duplicate {} issue #{} in one tracker response",
                provider.slug(),
                issue.external_id
            )),
            Err(error) => warnings.push(error.to_string()),
        }
    }

    (issues, warnings)
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
        let owner = encode_url_component(owner);
        let repo = encode_url_component(repo);
        let mut issues = Vec::new();
        for page in 1..=1_000 {
            let path = match provider {
                Provider::GitHub => {
                    format!("/repos/{owner}/{repo}/issues?state=all&per_page=100&page={page}")
                }
                Provider::Gitea => {
                    format!("/repos/{owner}/{repo}/issues?state=all&limit=100&page={page}")
                }
            };
            let url = format!("{}{}", self.api_base, path);
            let resp = send_get_with_retry("tracker GET", GET_RETRY_POLICY, || {
                let mut req = self.http.get(&url).header("accept", "application/json");
                if let Some(token) = &self.token {
                    req = req.bearer_auth(token);
                }
                req
            })
            .await
            .with_context(|| format!("GET {url}"))?;
            let (status, text) =
                read_response_text_limited(resp, MAX_SYNC_RESPONSE_BYTES, "tracker API").await?;
            if !status.is_success() {
                return Err(anyhow!(
                    "tracker API {status}: {}",
                    error_body_preview(&text)
                ));
            }
            let value: Value = serde_json::from_str(&text).context("parse tracker JSON")?;
            // GitHub returns a bare array; some Gitea deployments wrap in {data:[]}.
            let page_items = decode_collection(value, "data", "tracker issue response")?;
            let page_size = page_items.len();
            // GitHub's issues endpoint also returns pull requests; skip those.
            issues.extend(
                page_items
                    .into_iter()
                    .filter(|issue| issue.get("pull_request").is_none()),
            );
            if page_size < 100 {
                return Ok(issues);
            }
        }
        Err(anyhow!(
            "tracker pagination exceeded 1000 pages; refusing to return a truncated issue list"
        ))
    }
}

/// Minimal Tea HTTP API client (bearer auth, JSON).
struct TeaClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Debug, PartialEq, Eq)]
enum CreateTicketOutcome {
    Created(String),
    Conflict,
}

impl TeaClient {
    async fn list_tickets(&self) -> Result<Vec<Value>> {
        let mut tickets = Vec::new();
        let mut ticket_indexes = BTreeMap::new();
        let mut seen_cursors = HashSet::new();
        let mut cursor: Option<String> = None;

        for page_number in 1..=MAX_TEA_TICKET_PAGES {
            let mut url = format!("{}/v1/tickets?limit={TEA_TICKET_PAGE_SIZE}", self.base_url);
            if let Some(cursor) = &cursor {
                url.push_str("&cursor=");
                url.push_str(&encode_url_component(cursor));
            }
            let resp = send_get_with_retry("Tea ticket GET", GET_RETRY_POLICY, || {
                self.http.get(&url).bearer_auth(&self.token)
            })
            .await?;
            let (status, text) =
                read_response_text_limited(resp, MAX_SYNC_RESPONSE_BYTES, "Tea ticket list")
                    .await?;
            if !status.is_success() {
                return Err(anyhow!(
                    "Tea list tickets {status}: {}",
                    error_body_preview(&text)
                ));
            }
            let value: Value = serde_json::from_str(&text)?;
            match decode_tea_ticket_page(value)? {
                TeaTicketPage::Legacy(items) => {
                    append_unique_tickets(&mut tickets, &mut ticket_indexes, items)?;
                    return Ok(tickets);
                }
                TeaTicketPage::Paged { items, next_cursor } => {
                    if items.len() > TEA_TICKET_PAGE_SIZE {
                        return Err(anyhow!(
                            "Tea ticket page {page_number} returned {} items, exceeding the requested {TEA_TICKET_PAGE_SIZE}",
                            items.len()
                        ));
                    }
                    if items.is_empty() && next_cursor.is_some() {
                        return Err(anyhow!(
                            "Tea ticket page {page_number} returned an empty page with a continuation cursor"
                        ));
                    }
                    append_unique_tickets(&mut tickets, &mut ticket_indexes, items)?;
                    let Some(next_cursor) = next_cursor else {
                        return Ok(tickets);
                    };
                    if !seen_cursors.insert(next_cursor.clone()) {
                        return Err(anyhow!(
                            "Tea ticket pagination repeated cursor {next_cursor}; refusing a partial snapshot"
                        ));
                    }
                    cursor = Some(next_cursor);
                }
            }
        }

        Err(anyhow!(
            "Tea ticket pagination exceeded {MAX_TEA_TICKET_PAGES} pages; refusing to return a truncated ticket list"
        ))
    }

    async fn create_ticket(
        &self,
        body: &Value,
        idempotency_key: &str,
    ) -> Result<CreateTicketOutcome> {
        let url = format!("{}/v1/tickets", self.base_url);
        let response = self
            .http
            .post(&url)
            .header("Idempotency-Key", idempotency_key)
            .json(body)
            .bearer_auth(&self.token)
            .send()
            .await?;
        let (status, text) =
            read_response_text_limited(response, MAX_SYNC_RESPONSE_BYTES, "Tea create API").await?;
        if status == StatusCode::CONFLICT {
            return Ok(CreateTicketOutcome::Conflict);
        }
        if !status.is_success() {
            return Err(anyhow!("Tea API {status}: {}", error_body_preview(&text)));
        }
        let response: Value = serde_json::from_str(&text).context("decode Tea create response")?;
        response
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(|id| CreateTicketOutcome::Created(id.to_string()))
            .context("Tea create response did not include a non-empty ticket id")
    }

    async fn edit_ticket(&self, ticket_id: &str, body: &Value) -> Result<()> {
        let url = format!(
            "{}/v1/tickets/{}",
            self.base_url,
            encode_url_component(ticket_id)
        );
        self.send(self.http.patch(&url).json(body)).await
    }

    /// Returns true when this request applied the transition and false when a
    /// conflict readback proves that the ticket already has the target status.
    async fn lifecycle(&self, ticket_id: &str, action: &str) -> Result<bool> {
        let target_status = lifecycle_target_status(action)?;
        let url = format!(
            "{}/v1/tickets/{}/{}",
            self.base_url,
            encode_url_component(ticket_id),
            action
        );
        let resp = self.http.post(&url).bearer_auth(&self.token).send().await?;
        let (status, text) =
            read_response_text_limited(resp, MAX_SYNC_RESPONSE_BYTES, "Tea lifecycle API").await?;
        match interpret_lifecycle_response(status, &text)? {
            LifecycleResponse::Applied => Ok(true),
            LifecycleResponse::Conflict { preview } => {
                let observed_status = self.ticket_status(ticket_id).await.map_err(|error| {
                    anyhow!(
                        "Tea lifecycle API 409 Conflict: {preview}; failed to confirm ticket {ticket_id} status after {action}: {error}"
                    )
                })?;
                if observed_status == target_status {
                    return Ok(false);
                }
                Err(anyhow!(
                    "Tea lifecycle API 409 Conflict: {preview}; ticket {ticket_id} has status {observed_status:?}, expected {target_status:?} after {action}"
                ))
            }
        }
    }

    async fn ticket_status(&self, ticket_id: &str) -> Result<String> {
        let url = format!(
            "{}/v1/tickets/{}",
            self.base_url,
            encode_url_component(ticket_id)
        );
        let resp = send_get_with_retry("Tea ticket status GET", GET_RETRY_POLICY, || {
            self.http.get(&url).bearer_auth(&self.token)
        })
        .await?;
        let (status, text) =
            read_response_text_limited(resp, MAX_SYNC_RESPONSE_BYTES, "Tea ticket status API")
                .await?;
        if !status.is_success() {
            return Err(anyhow!(
                "Tea ticket status API {status}: {}",
                error_body_preview(&text)
            ));
        }
        let ticket: Value =
            serde_json::from_str(&text).context("decode Tea ticket status response")?;
        ticket
            .get("status")
            .and_then(Value::as_str)
            .filter(|status| !status.is_empty())
            .map(str::to_string)
            .context("Tea ticket status response did not include a non-empty status")
    }

    async fn send(&self, req: reqwest::RequestBuilder) -> Result<()> {
        let resp = req.bearer_auth(&self.token).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let (_, text) =
                read_response_text_limited(resp, MAX_SYNC_RESPONSE_BYTES, "Tea API").await?;
            return Err(anyhow!("Tea API {status}: {}", error_body_preview(&text)));
        }
        Ok(())
    }
}

enum TeaTicketPage {
    Legacy(Vec<Value>),
    Paged {
        items: Vec<Value>,
        next_cursor: Option<String>,
    },
}

fn decode_tea_ticket_page(value: Value) -> Result<TeaTicketPage> {
    match value {
        Value::Array(items) => Ok(TeaTicketPage::Legacy(items)),
        Value::Object(mut object) => {
            if let Some(items) = object.remove("items") {
                let Value::Array(items) = items else {
                    return Err(anyhow!(
                        "Tea ticket response field 'items' must be a JSON array"
                    ));
                };
                let next_cursor = match object.remove("next_cursor") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(cursor)) if !cursor.is_empty() => Some(cursor),
                    Some(_) => {
                        return Err(anyhow!(
                            "Tea ticket response field 'next_cursor' must be a non-empty string or null"
                        ));
                    }
                };
                return Ok(TeaTicketPage::Paged { items, next_cursor });
            }
            match object.remove("tickets") {
                Some(Value::Array(items)) => Ok(TeaTicketPage::Legacy(items)),
                Some(_) => Err(anyhow!(
                    "Tea ticket response field 'tickets' must be a JSON array"
                )),
                None => Err(anyhow!(
                    "Tea ticket response must be an array or contain an 'items' array"
                )),
            }
        }
        _ => Err(anyhow!(
            "Tea ticket response must be a JSON array or an object envelope"
        )),
    }
}

fn append_unique_tickets(
    tickets: &mut Vec<Value>,
    ticket_indexes: &mut BTreeMap<String, usize>,
    page_items: Vec<Value>,
) -> Result<()> {
    for ticket in page_items {
        let id = ticket
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| anyhow!("Tea ticket page item is missing a non-empty string id"))?
            .to_string();
        if let Some(index) = ticket_indexes.get(&id).copied() {
            if tickets[index] != ticket {
                return Err(anyhow!(
                    "Tea ticket pagination returned conflicting records for ticket {id}"
                ));
            }
            continue;
        }
        ticket_indexes.insert(id, tickets.len());
        tickets.push(ticket);
    }
    Ok(())
}

/// Bounded response read via the shared `tea_http_read` helper, keeping this
/// binary's historical extra context on mid-body transport errors.
async fn read_response_text_limited(
    response: reqwest::Response,
    max_bytes: usize,
    context: &str,
) -> Result<(StatusCode, String)> {
    tea_http_read::read_response_text_limited(response, max_bytes, context)
        .await
        .map_err(|error| match error {
            error @ ReadLimitedError::Read(_) => {
                anyhow::Error::new(error).context(format!("read {context} response body"))
            }
            error => anyhow::Error::new(error),
        })
}

async fn send_get_with_retry<F>(
    context: &str,
    policy: GetRetryPolicy,
    mut request: F,
) -> Result<reqwest::Response>
where
    F: FnMut() -> reqwest::RequestBuilder,
{
    let started = std::time::Instant::now();
    let max_attempts = policy.max_attempts.max(1);

    for attempt in 1..=max_attempts {
        let remaining = policy.total_timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(anyhow!(
                "{context} exceeded its {:?} total retry timeout",
                policy.total_timeout
            ));
        }

        match tokio::time::timeout(remaining, request().send()).await {
            Ok(Ok(response)) => {
                if attempt == max_attempts || !is_retryable_get_status(response.status()) {
                    return Ok(response);
                }
                let delay = get_retry_delay(attempt, response.headers(), policy);
                eprintln!(
                    "{context} returned {}; retrying attempt {}/{} after {:?}",
                    response.status(),
                    attempt + 1,
                    max_attempts,
                    delay
                );
                sleep_within_retry_budget(context, delay, started, policy).await?;
            }
            Ok(Err(error)) => {
                if attempt == max_attempts || !is_retryable_get_error(&error) {
                    return Err(error).with_context(|| {
                        format!("{context} failed on attempt {attempt}/{max_attempts}")
                    });
                }
                let delay = get_retry_delay(attempt, &reqwest::header::HeaderMap::new(), policy);
                eprintln!(
                    "{context} transport failure; retrying attempt {}/{} after {:?}: {}",
                    attempt + 1,
                    max_attempts,
                    delay,
                    error
                );
                sleep_within_retry_budget(context, delay, started, policy).await?;
            }
            Err(_) => {
                return Err(anyhow!(
                    "{context} exceeded its {:?} total retry timeout",
                    policy.total_timeout
                ));
            }
        }
    }

    unreachable!("GET retry loop always returns")
}

async fn sleep_within_retry_budget(
    context: &str,
    delay: Duration,
    started: std::time::Instant,
    policy: GetRetryPolicy,
) -> Result<()> {
    let remaining = policy.total_timeout.saturating_sub(started.elapsed());
    if delay >= remaining && !delay.is_zero() {
        return Err(anyhow!(
            "{context} exhausted its {:?} total retry timeout",
            policy.total_timeout
        ));
    }
    tokio::time::sleep(delay).await;
    Ok(())
}

fn is_retryable_get_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_EARLY
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}

fn is_retryable_get_error(error: &reqwest::Error) -> bool {
    error.is_connect() || error.is_timeout() || error.is_request() || error.is_body()
}

fn get_retry_delay(
    failed_attempt: usize,
    headers: &reqwest::header::HeaderMap,
    policy: GetRetryPolicy,
) -> Duration {
    if let Some(retry_after) = headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
    {
        return Duration::from_secs(retry_after).min(policy.max_delay);
    }

    let exponent = failed_attempt.saturating_sub(1).min(20) as u32;
    policy
        .base_delay
        .saturating_mul(1_u32 << exponent)
        .min(policy.max_delay)
}

fn decode_collection(value: Value, envelope_key: &str, context: &str) -> Result<Vec<Value>> {
    match value {
        Value::Array(items) => Ok(items),
        Value::Object(mut object) => match object.remove(envelope_key) {
            Some(Value::Array(items)) => Ok(items),
            Some(_) => Err(anyhow!(
                "{context} field '{envelope_key}' must be a JSON array"
            )),
            None => Err(anyhow!(
                "{context} must be an array or contain an '{envelope_key}' array"
            )),
        },
        _ => Err(anyhow!(
            "{context} must be a JSON array or an object envelope"
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum LifecycleResponse {
    Applied,
    Conflict { preview: String },
}

fn lifecycle_target_status(action: &str) -> Result<&'static str> {
    match action {
        "cancel" => Ok("cancelled"),
        "close" => Ok("closed"),
        _ => Err(anyhow!("unsupported Tea lifecycle action {action}")),
    }
}

fn interpret_lifecycle_response(status: StatusCode, body: &str) -> Result<LifecycleResponse> {
    if status.is_success() {
        return Ok(LifecycleResponse::Applied);
    }
    if status == StatusCode::CONFLICT {
        return Ok(LifecycleResponse::Conflict {
            preview: error_body_preview(body),
        });
    }
    Err(anyhow!(
        "Tea lifecycle API {status}: {}",
        error_body_preview(body)
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        append_unique_tickets, build_http_client, decode_collection, decode_tea_ticket_page,
        decode_ticket_refs, encode_url_component, get_retry_delay, interpret_lifecycle_response,
        is_retryable_get_status, lifecycle_target_status, parse_unique_issues,
        read_response_text_limited, send_get_with_retry, CreateTicketOutcome, GetRetryPolicy,
        LifecycleResponse, TeaClient, TeaTicketPage,
    };
    use reqwest::StatusCode;
    use serde_json::json;
    use std::{
        io::{Read, Write},
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::Duration,
    };
    use tea_sync::Provider;

    fn test_retry_policy() -> GetRetryPolicy {
        GetRetryPolicy {
            max_attempts: 3,
            base_delay: Duration::ZERO,
            max_delay: Duration::ZERO,
            total_timeout: Duration::from_secs(5),
        }
    }

    fn spawn_http_server(
        responses: Vec<(&str, &str)>,
    ) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
        let responses = responses
            .into_iter()
            .map(|(status, body)| (status.to_string(), body.to_string()))
            .collect::<Vec<_>>();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let request_count = Arc::new(AtomicUsize::new(0));
        let server_count = Arc::clone(&request_count);
        let server = std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request).unwrap();
                server_count.fetch_add(1, Ordering::SeqCst);
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                stream.flush().unwrap();
            }
        });
        (format!("http://{address}"), request_count, server)
    }

    fn spawn_capturing_http_server(
        response_body: &'static str,
    ) -> (
        String,
        std::sync::mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            sender
                .send(String::from_utf8_lossy(&request[..read]).to_string())
                .unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .unwrap();
            stream.flush().unwrap();
        });
        (format!("http://{address}"), receiver, server)
    }

    fn spawn_capturing_http_server_sequence(
        responses: Vec<(&str, &str)>,
    ) -> (
        String,
        std::sync::mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        let responses = responses
            .into_iter()
            .map(|(status, body)| (status.to_string(), body.to_string()))
            .collect::<Vec<_>>();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let read = stream.read(&mut request).unwrap();
                sender
                    .send(String::from_utf8_lossy(&request[..read]).to_string())
                    .unwrap();
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
                stream.flush().unwrap();
            }
        });
        (format!("http://{address}"), receiver, server)
    }

    #[test]
    fn url_components_encode_utf8_bytes() {
        assert_eq!(
            encode_url_component("中文/é"),
            "%E4%B8%AD%E6%96%87%2F%C3%A9"
        );
    }

    #[test]
    fn collection_decoder_rejects_malformed_success_envelopes() {
        assert!(decode_collection(json!({}), "tickets", "Tea response").is_err());
        assert!(decode_collection(json!({ "tickets": null }), "tickets", "Tea response").is_err());
        assert!(decode_collection(json!("not-a-list"), "tickets", "Tea response").is_err());
        assert_eq!(
            decode_collection(
                json!({ "tickets": [{ "id": "1" }] }),
                "tickets",
                "Tea response"
            )
            .unwrap()
            .len(),
            1
        );
    }

    #[test]
    fn tea_ticket_page_decoder_preserves_legacy_and_validates_envelopes() {
        match decode_tea_ticket_page(json!([{"id":"legacy"}])).unwrap() {
            TeaTicketPage::Legacy(items) => assert_eq!(items.len(), 1),
            TeaTicketPage::Paged { .. } => panic!("bare arrays must remain legacy pages"),
        }
        match decode_tea_ticket_page(json!({
            "items":[{"id":"paged"}],
            "next_cursor":"v1-0000000000000000-0-0"
        }))
        .unwrap()
        {
            TeaTicketPage::Paged { items, next_cursor } => {
                assert_eq!(items.len(), 1);
                assert_eq!(next_cursor.as_deref(), Some("v1-0000000000000000-0-0"));
            }
            TeaTicketPage::Legacy(_) => panic!("items envelopes must be paged"),
        }
        assert!(decode_tea_ticket_page(json!({"items":null})).is_err());
        assert!(decode_tea_ticket_page(json!({"items":[],"next_cursor":""})).is_err());
        assert!(decode_tea_ticket_page(json!({})).is_err());
    }

    #[test]
    fn ticket_page_merge_rejects_conflicting_duplicate_ids() {
        let mut tickets = Vec::new();
        let mut indexes = std::collections::BTreeMap::new();
        append_unique_tickets(
            &mut tickets,
            &mut indexes,
            vec![json!({"id":"same","title":"first"})],
        )
        .unwrap();
        append_unique_tickets(
            &mut tickets,
            &mut indexes,
            vec![json!({"id":"same","title":"first"})],
        )
        .unwrap();
        assert_eq!(tickets.len(), 1);
        assert!(append_unique_tickets(
            &mut tickets,
            &mut indexes,
            vec![json!({"id":"same","title":"changed"})],
        )
        .is_err());
    }

    #[test]
    fn tea_ticket_refs_reject_malformed_ids_and_labels() {
        assert!(decode_ticket_refs(&[json!({"labels":[]})]).is_err());
        assert!(decode_ticket_refs(&[json!({"id":"ticket","labels":null})]).is_err());
        assert!(decode_ticket_refs(&[json!({"id":"ticket","labels":[1]})]).is_err());
        assert_eq!(
            decode_ticket_refs(&[json!({
                "id":"ticket",
                "labels":["sync-id:github:42"]
            })])
            .unwrap(),
            vec![("ticket".to_string(), vec!["sync-id:github:42".to_string()])]
        );
    }

    #[tokio::test]
    async fn tea_client_collects_cursor_pages() {
        let (url, request_count, server) = spawn_http_server(vec![
            (
                "200 OK",
                r#"{"items":[{"id":"one"}],"next_cursor":"v1-0000000000000000-0-0"}"#,
            ),
            ("200 OK", r#"{"items":[{"id":"two"}],"next_cursor":null}"#),
        ]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let tickets = client.list_tickets().await.unwrap();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
        assert_eq!(tickets.len(), 2);
        assert_eq!(tickets[0]["id"], "one");
        assert_eq!(tickets[1]["id"], "two");
    }

    #[tokio::test]
    async fn tea_client_rejects_repeated_cursors() {
        let (url, request_count, server) = spawn_http_server(vec![
            (
                "200 OK",
                r#"{"items":[{"id":"one"}],"next_cursor":"repeated"}"#,
            ),
            (
                "200 OK",
                r#"{"items":[{"id":"two"}],"next_cursor":"repeated"}"#,
            ),
        ]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let error = client.list_tickets().await.unwrap_err();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
        assert!(error.to_string().contains("repeated cursor"));
    }

    #[tokio::test]
    async fn tea_sync_create_returns_ticket_id_and_sends_deterministic_idempotency_key() {
        let (url, request, server) = spawn_capturing_http_server(r#"{"id":"created-ticket"}"#);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let outcome = client
            .create_ticket(
                &json!({
                    "title": "Sync header test",
                    "description": "Verify deterministic sync key propagation."
                }),
                "tea-sync-v1-github-testhash",
            )
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(
            outcome,
            CreateTicketOutcome::Created("created-ticket".to_string())
        );
        let request = request.recv().unwrap().to_ascii_lowercase();
        assert!(request.contains("idempotency-key: tea-sync-v1-github-testhash\r\n"));
        assert!(request.contains("authorization: bearer test-token\r\n"));
    }

    #[tokio::test]
    async fn tea_sync_create_surfaces_idempotency_conflict_for_reconciliation() {
        let (url, request_count, server) = spawn_http_server(vec![(
            "409 Conflict",
            r#"{"error":"idempotency key was already used with a different request"}"#,
        )]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let outcome = client
            .create_ticket(
                &json!({
                    "title": "Changed sync payload",
                    "description": "Recover the already-created mirror."
                }),
                "tea-sync-v1-github-testhash",
            )
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert_eq!(outcome, CreateTicketOutcome::Conflict);
    }

    #[test]
    fn lifecycle_response_defers_only_conflicts_for_confirmation() {
        assert_eq!(
            interpret_lifecycle_response(StatusCode::OK, "").unwrap(),
            LifecycleResponse::Applied
        );
        assert_eq!(
            interpret_lifecycle_response(StatusCode::CONFLICT, "terminal").unwrap(),
            LifecycleResponse::Conflict {
                preview: "terminal".to_string()
            }
        );
        assert!(interpret_lifecycle_response(StatusCode::UNAUTHORIZED, "bad token").is_err());
        assert!(interpret_lifecycle_response(StatusCode::INTERNAL_SERVER_ERROR, "boom").is_err());
    }

    #[test]
    fn lifecycle_target_statuses_are_explicit() {
        assert_eq!(lifecycle_target_status("cancel").unwrap(), "cancelled");
        assert_eq!(lifecycle_target_status("close").unwrap(), "closed");
        assert!(lifecycle_target_status("approve").is_err());
    }

    #[tokio::test]
    async fn lifecycle_success_does_not_read_back_ticket_status() {
        let (url, request_count, server) = spawn_http_server(vec![("200 OK", "")]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        assert!(client.lifecycle("ticket", "cancel").await.unwrap());

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn lifecycle_conflict_is_idempotent_only_at_the_target_status() {
        let (url, requests, server) = spawn_capturing_http_server_sequence(vec![
            ("409 Conflict", r#"{"error":"ticket is already cancelled"}"#),
            ("200 OK", r#"{"status":"cancelled"}"#),
        ]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        assert!(!client.lifecycle("ticket", "cancel").await.unwrap());

        server.join().unwrap();
        let lifecycle_request = requests.recv().unwrap().to_ascii_lowercase();
        let readback_request = requests.recv().unwrap().to_ascii_lowercase();
        assert!(lifecycle_request.starts_with("post /v1/tickets/ticket/cancel http/1.1\r\n"));
        assert!(readback_request.starts_with("get /v1/tickets/ticket http/1.1\r\n"));
        assert!(lifecycle_request.contains("authorization: bearer test-token\r\n"));
        assert!(readback_request.contains("authorization: bearer test-token\r\n"));
    }

    #[tokio::test]
    async fn lifecycle_conflict_fails_when_the_ticket_has_another_status() {
        let (url, request_count, server) = spawn_http_server(vec![
            ("409 Conflict", r#"{"error":"invalid transition"}"#),
            ("200 OK", r#"{"status":"open"}"#),
        ]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let error = client.lifecycle("ticket", "cancel").await.unwrap_err();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
        let message = error.to_string();
        assert!(message.contains("has status \"open\""));
        assert!(message.contains("expected \"cancelled\""));
    }

    #[tokio::test]
    async fn lifecycle_conflict_fails_when_status_readback_is_malformed() {
        let (url, request_count, server) = spawn_http_server(vec![
            (
                "409 Conflict",
                r#"{"error":"approval requirements were not met"}"#,
            ),
            ("200 OK", r#"{"status":7}"#),
        ]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let error = client.lifecycle("ticket", "cancel").await.unwrap_err();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
        let message = error.to_string();
        assert!(message.contains("approval requirements were not met"));
        assert!(message.contains("did not include a non-empty status"));
    }

    #[tokio::test]
    async fn lifecycle_conflict_fails_when_status_readback_is_rejected() {
        let (url, request_count, server) = spawn_http_server(vec![
            ("409 Conflict", r#"{"error":"invalid transition"}"#),
            ("401 Unauthorized", r#"{"error":"readback denied"}"#),
        ]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let error = client.lifecycle("ticket", "cancel").await.unwrap_err();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
        let message = error.to_string();
        assert!(message.contains("invalid transition"));
        assert!(message.contains("Tea ticket status API 401 Unauthorized"));
        assert!(message.contains("readback denied"));
    }

    #[tokio::test]
    async fn lifecycle_non_conflict_failure_does_not_read_back_ticket_status() {
        let (url, request_count, server) =
            spawn_http_server(vec![("401 Unauthorized", "bad token")]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let error = client.lifecycle("ticket", "cancel").await.unwrap_err();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert!(error.to_string().contains("401 Unauthorized"));
    }

    #[tokio::test]
    async fn lifecycle_conflict_error_keeps_only_a_bounded_body_preview() {
        let conflict_body = "x".repeat(600);
        let (url, request_count, server) = spawn_http_server(vec![
            ("409 Conflict", &conflict_body),
            ("200 OK", r#"{"status":"open"}"#),
        ]);
        let client = TeaClient {
            base_url: url,
            token: "test-token".to_string(),
            http: build_http_client().unwrap(),
        };

        let error = client.lifecycle("ticket", "cancel").await.unwrap_err();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
        let message = error.to_string();
        assert!(message.contains(&format!("{}...", "x".repeat(512))));
        assert!(!message.contains(&"x".repeat(513)));
    }

    #[test]
    fn get_retry_statuses_are_narrowly_classified() {
        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::TOO_EARLY,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(is_retryable_get_status(status), "{status}");
        }
        for status in [
            StatusCode::BAD_REQUEST,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::CONFLICT,
        ] {
            assert!(!is_retryable_get_status(status), "{status}");
        }
    }

    #[test]
    fn retry_after_seconds_are_capped() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "30".parse().unwrap());
        let policy = GetRetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            total_timeout: Duration::from_secs(5),
        };

        assert_eq!(get_retry_delay(1, &headers, policy), Duration::from_secs(2));
    }

    #[tokio::test]
    async fn idempotent_get_retries_a_transient_server_failure() {
        let (url, request_count, server) =
            spawn_http_server(vec![("503 Service Unavailable", "retry"), ("200 OK", "[]")]);
        let client = reqwest::Client::new();

        let response = send_get_with_retry("test GET", test_retry_policy(), || client.get(&url))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.text().await.unwrap(), "[]");
        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn idempotent_get_does_not_retry_authentication_failures() {
        let (url, request_count, server) =
            spawn_http_server(vec![("401 Unauthorized", "bad token")]);
        let client = reqwest::Client::new();

        let response = send_get_with_retry("test GET", test_retry_policy(), || client.get(&url))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn response_reader_rejects_an_oversized_body() {
        let (url, request_count, server) = spawn_http_server(vec![("200 OK", "123456789")]);
        let response = reqwest::Client::new().get(url).send().await.unwrap();

        let error = read_response_text_limited(response, 8, "test API")
            .await
            .unwrap_err();

        server.join().unwrap();
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert!(error.to_string().contains("8-byte limit"));
    }

    #[tokio::test]
    async fn sync_http_client_does_not_follow_redirects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:1/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            stream.flush().unwrap();
        });

        let response = build_http_client()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
    }

    #[test]
    fn duplicate_and_malformed_issues_are_skipped_before_planning() {
        let raw = vec![
            json!({ "number": 7, "title": "First", "state": "open" }),
            json!({ "number": 7, "title": "Duplicate", "state": "closed" }),
            json!({ "number": 8, "state": "open" }),
        ];

        let (issues, warnings) = parse_unique_issues(Provider::GitHub, &raw);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].external_id, "7");
        assert_eq!(issues[0].title, "First");
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("duplicate github issue #7"));
        assert!(warnings[1].contains("missing required field: title"));
    }
}
