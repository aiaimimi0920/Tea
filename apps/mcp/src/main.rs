#![forbid(unsafe_code)]

//! Tea MCP server.
//!
//! Exposes Tea ticket operations as Model Context Protocol tools over stdio so
//! MCP-capable agents can create, review, and drive Tea work orders. The server
//! is a thin adapter: `tea_mcp` owns the tool catalog and request routing, and
//! this binary owns stdio framing plus HTTP execution against the Tea daemon.
//!
//! Transport: newline-delimited JSON-RPC 2.0 over stdin/stdout (one JSON value
//! per line), matching the stdio MCP convention used elsewhere in Neuro.

use std::io::{BufRead, Write};

use clap::Parser;
use serde_json::{json, Value};
use tea_mcp::{
    initialize_result, jsonrpc_error, jsonrpc_result, resolve_tool_call, tool_call_result,
    tools_list_result, HttpMethod, TeaAction,
};

#[derive(Debug, Parser)]
#[command(name = "tea-mcp", about = "Tea Model Context Protocol server (stdio)")]
struct Cli {
    /// Base URL of the Tea daemon HTTP API.
    #[arg(long, env = "TEA_SERVER_URL", default_value = "http://127.0.0.1:48910")]
    server_url: String,
    /// Bearer token for the Tea daemon HTTP API.
    #[arg(long, env = "TEA_AUTH_TOKEN", default_value = "dev-token")]
    auth_token: String,
}

/// JSON-RPC parse error code per the spec.
const PARSE_ERROR: i64 = -32700;
/// JSON-RPC method-not-found code per the spec.
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC invalid-params code per the spec.
const INVALID_PARAMS: i64 = -32602;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = TeaHttpClient::new(cli.server_url, cli.auth_token);

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // Read one JSON-RPC message per line. Blank lines are ignored so the stream
    // stays resilient to pretty-printer newlines from simple clients.
    for line in stdin.lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let Some(response) = handle_line(trimmed, &client).await else {
            // Notifications (no id) produce no response.
            continue;
        };

        let serialized = serde_json::to_string(&response)?;
        stdout.write_all(serialized.as_bytes())?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }

    Ok(())
}

/// Handle a single JSON-RPC line, returning `Some(response)` for requests and
/// `None` for notifications or unparseable input that carries no id.
async fn handle_line(line: &str, client: &TeaHttpClient) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return Some(jsonrpc_error(
                Value::Null,
                PARSE_ERROR,
                format!("invalid JSON-RPC message: {error}"),
            ));
        }
    };

    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");

    // Notifications (no id) are acknowledged silently.
    let Some(id) = id else {
        return None;
    };

    match method {
        "initialize" => Some(jsonrpc_result(id, initialize_result())),
        "tools/list" => Some(jsonrpc_result(id, tools_list_result())),
        "tools/call" => Some(handle_tools_call(id, &message, client).await),
        "ping" => Some(jsonrpc_result(id, json!({}))),
        other => Some(jsonrpc_error(
            id,
            METHOD_NOT_FOUND,
            format!("method not supported: {other}"),
        )),
    }
}

async fn handle_tools_call(id: Value, message: &Value, client: &TeaHttpClient) -> Value {
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let action = match resolve_tool_call(name, &arguments) {
        Ok(action) => action,
        Err(error) => {
            return jsonrpc_error(id, INVALID_PARAMS, error.to_string());
        }
    };

    match client.execute(&action).await {
        Ok(text) => jsonrpc_result(id, tool_call_result(text, false)),
        // Tool execution failures are reported inside a successful JSON-RPC
        // result with `isError: true`, per the MCP tools/call convention, so the
        // agent can read the error text rather than the whole call failing.
        Err(error) => jsonrpc_result(id, tool_call_result(error.to_string(), true)),
    }
}

/// Minimal HTTP client that executes a resolved [`TeaAction`] against the Tea
/// daemon with bearer auth, mirroring the tea-cli client behavior.
struct TeaHttpClient {
    base_url: String,
    token: String,
    http: reqwest::Client,
}

impl TeaHttpClient {
    fn new(base_url: String, token: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
            http: reqwest::Client::new(),
        }
    }

    async fn execute(&self, action: &TeaAction) -> anyhow::Result<String> {
        let url = format!("{}{}", self.base_url, action.path);
        let mut request = match action.method {
            HttpMethod::Get => self.http.get(&url),
            HttpMethod::Post => self.http.post(&url),
            HttpMethod::Patch => self.http.patch(&url),
        };
        request = request.bearer_auth(&self.token);
        if let Some(body) = &action.body {
            request = request.json(body);
        }

        let response = request.send().await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            anyhow::bail!("Tea API returned {status}: {body}");
        }

        if action.expects_text {
            return Ok(body);
        }

        // Pretty-print JSON responses so agents get readable tool output; fall
        // back to the raw body if the daemon returned non-JSON.
        match serde_json::from_str::<Value>(&body) {
            Ok(value) => Ok(serde_json::to_string_pretty(&value)?),
            Err(_) => Ok(body),
        }
    }
}
