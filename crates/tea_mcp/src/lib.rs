#![forbid(unsafe_code)]

//! Tea MCP server core.
//!
//! This crate exposes Tea's work-order operations as Model Context Protocol
//! (MCP) tools so AI agents can create, edit, comment on, review, and fetch
//! Tea tickets over a stdio JSON-RPC transport.
//!
//! The core here is deliberately pure and I/O-free: it defines the tool
//! catalog, translates an MCP `tools/call` into a [`TeaAction`] (an HTTP method
//! + path + optional JSON body against the Tea daemon), and builds JSON-RPC
//! responses. The actual HTTP execution and stdio loop live in the
//! `tea-mcp` binary so the mapping stays trivially testable.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;

/// Protocol version this server advertises during `initialize`.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Server name advertised to MCP clients.
pub const SERVER_NAME: &str = "tea-mcp";

/// Server version advertised to MCP clients.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Error, PartialEq, Eq)]
pub enum McpError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("missing required argument: {0}")]
    MissingArgument(&'static str),
    #[error("argument {name} must be a {expected}")]
    InvalidArgument {
        name: &'static str,
        expected: &'static str,
    },
    #[error("tool arguments must be a JSON object")]
    ArgumentsNotObject,
}

/// HTTP method for a resolved Tea action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Patch,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Patch => "PATCH",
        }
    }
}

/// A concrete Tea HTTP API action a tool call resolves to.
///
/// `path` is relative to the Tea server base URL (e.g. `/v1/tickets`).
/// `body` is `Some` for mutating requests that send a JSON payload.
#[derive(Debug, Clone, PartialEq)]
pub struct TeaAction {
    pub method: HttpMethod,
    pub path: String,
    pub body: Option<Value>,
    /// When true, the daemon returns text/markdown rather than JSON; the server
    /// should surface the raw text instead of pretty-printing JSON.
    pub expects_text: bool,
}

impl TeaAction {
    fn get(path: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            path: path.into(),
            body: None,
            expects_text: false,
        }
    }

    fn get_text(path: impl Into<String>) -> Self {
        Self {
            method: HttpMethod::Get,
            path: path.into(),
            body: None,
            expects_text: true,
        }
    }

    fn post(path: impl Into<String>, body: Value) -> Self {
        Self {
            method: HttpMethod::Post,
            path: path.into(),
            body: Some(body),
            expects_text: false,
        }
    }

    fn patch(path: impl Into<String>, body: Value) -> Self {
        Self {
            method: HttpMethod::Patch,
            path: path.into(),
            body: Some(body),
            expects_text: false,
        }
    }
}

/// A single MCP tool description for `tools/list`.
#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

fn ticket_id_schema() -> Value {
    object_schema(
        json!({
            "ticket_id": { "type": "string", "description": "Tea ticket id (UUID)." },
        }),
        &["ticket_id"],
    )
}

/// The full catalog of Tea MCP tools, in a stable order.
pub fn tool_catalog() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "tea_status",
            description: "Get Tea daemon service status and configuration source.",
            input_schema: object_schema(json!({}), &[]),
        },
        ToolSpec {
            name: "tea_list_tickets",
            description: "List Tea work-order tickets, optionally filtered by status or source.",
            input_schema: object_schema(
                json!({
                    "status": { "type": "string", "description": "Optional status filter (e.g. open, running, closed)." },
                    "source": { "type": "string", "description": "Optional source filter (human, hook, api, system)." },
                }),
                &[],
            ),
        },
        ToolSpec {
            name: "tea_get_ticket",
            description: "Fetch a single Tea ticket by id.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_create_ticket",
            description: "Create a new Tea work-order ticket. Human-created tickets default to the human_before_execute approval policy.",
            input_schema: object_schema(
                json!({
                    "title": { "type": "string", "description": "Ticket title (>= 3 characters)." },
                    "description": { "type": "string", "description": "Ticket description / body (>= 10 characters)." },
                    "priority": { "type": "string", "description": "Optional priority, e.g. high, normal, low." },
                    "labels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional initial operator labels.",
                    },
                    "approval_policy": { "type": "string", "description": "Optional approval policy override." },
                }),
                &["title", "description"],
            ),
        },
        ToolSpec {
            name: "tea_edit_ticket",
            description: "Edit a ticket's title, description, priority, and/or operator labels. System-derived labels are always preserved. Only provided fields change.",
            input_schema: object_schema(
                json!({
                    "ticket_id": { "type": "string", "description": "Tea ticket id (UUID)." },
                    "title": { "type": "string", "description": "New title." },
                    "description": { "type": "string", "description": "New description." },
                    "priority": { "type": "string", "description": "New priority." },
                    "labels": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Replacement operator labels (system labels are preserved by the daemon).",
                    },
                }),
                &["ticket_id"],
            ),
        },
        ToolSpec {
            name: "tea_comment_ticket",
            description: "Add a human/agent review comment to a Tea ticket.",
            input_schema: object_schema(
                json!({
                    "ticket_id": { "type": "string", "description": "Tea ticket id (UUID)." },
                    "body": { "type": "string", "description": "Comment body (non-empty)." },
                }),
                &["ticket_id", "body"],
            ),
        },
        ToolSpec {
            name: "tea_list_events",
            description: "List the durable event timeline for a Tea ticket.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_list_comments",
            description: "List review comments on a Tea ticket.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_analyze_ticket",
            description: "Run AI analysis on a Tea ticket and record the analysis.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_decompose_ticket",
            description: "Decompose a Tea ticket into a structured analysis and plan.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_plan_ticket",
            description: "Generate an execution plan for a Tea ticket and record it.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_approve_ticket",
            description: "Approve a Tea ticket so it may run under policy.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_reject_ticket",
            description: "Reject a Tea ticket's approval with a reason.",
            input_schema: object_schema(
                json!({
                    "ticket_id": { "type": "string", "description": "Tea ticket id (UUID)." },
                    "reason": { "type": "string", "description": "Reason for rejection (non-empty)." },
                }),
                &["ticket_id", "reason"],
            ),
        },
        ToolSpec {
            name: "tea_run_ticket",
            description: "Dispatch a Tea ticket to Loom for execution (policy-gated).",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_accept_ticket",
            description: "Record human acceptance of a completed Tea ticket.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_close_ticket",
            description: "Close a Tea ticket (requires evidence).",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_cancel_ticket",
            description: "Cancel a Tea ticket, moving it to a terminal cancelled state.",
            input_schema: ticket_id_schema(),
        },
        ToolSpec {
            name: "tea_export_ticket",
            description: "Export a Tea ticket timeline/evidence as JSON or Markdown.",
            input_schema: object_schema(
                json!({
                    "ticket_id": { "type": "string", "description": "Tea ticket id (UUID)." },
                    "format": {
                        "type": "string",
                        "enum": ["json", "markdown"],
                        "description": "Export format (default json).",
                    },
                }),
                &["ticket_id"],
            ),
        },
    ]
}

/// Names of every tool in the catalog, in catalog order.
pub fn tool_names() -> Vec<&'static str> {
    tool_catalog().into_iter().map(|tool| tool.name).collect()
}

fn require_object(arguments: &Value) -> Result<&serde_json::Map<String, Value>, McpError> {
    match arguments {
        Value::Object(map) => Ok(map),
        Value::Null => Err(McpError::ArgumentsNotObject),
        _ => Err(McpError::ArgumentsNotObject),
    }
}

fn require_str(
    map: &serde_json::Map<String, Value>,
    name: &'static str,
) -> Result<String, McpError> {
    match map.get(name) {
        None => Err(McpError::MissingArgument(name)),
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Err(McpError::MissingArgument(name))
            } else {
                Ok(trimmed.to_string())
            }
        }
        Some(_) => Err(McpError::InvalidArgument {
            name,
            expected: "string",
        }),
    }
}

fn optional_str(map: &serde_json::Map<String, Value>, name: &'static str) -> Option<String> {
    match map.get(name) {
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.trim().to_string()),
        _ => None,
    }
}

fn optional_labels(
    map: &serde_json::Map<String, Value>,
    name: &'static str,
) -> Result<Option<Vec<String>>, McpError> {
    match map.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut labels = Vec::new();
            for item in items {
                match item {
                    Value::String(value) => {
                        let trimmed = value.trim();
                        if !trimmed.is_empty() {
                            labels.push(trimmed.to_string());
                        }
                    }
                    _ => {
                        return Err(McpError::InvalidArgument {
                            name,
                            expected: "array of strings",
                        })
                    }
                }
            }
            Ok(Some(labels))
        }
        Some(_) => Err(McpError::InvalidArgument {
            name,
            expected: "array of strings",
        }),
    }
}

fn encode_ticket_path(ticket_id: &str, suffix: &str) -> String {
    // Ticket ids are UUIDs from the daemon; guard against odd input by
    // percent-encoding characters that would break the path. Keep it minimal
    // and dependency-free since UUIDs need no encoding in practice.
    let encoded: String = ticket_id
        .chars()
        .flat_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => vec![c],
            other => format!("%{:02X}", other as u32).chars().collect(),
        })
        .collect();
    format!("/v1/tickets/{encoded}{suffix}")
}

/// Translate an MCP `tools/call` (tool name + arguments) into a [`TeaAction`].
pub fn resolve_tool_call(name: &str, arguments: &Value) -> Result<TeaAction, McpError> {
    match name {
        "tea_status" => Ok(TeaAction::get("/v1/status")),
        "tea_list_tickets" => {
            // Arguments are optional for listing.
            let map = match arguments {
                Value::Null => serde_json::Map::new(),
                Value::Object(map) => map.clone(),
                _ => return Err(McpError::ArgumentsNotObject),
            };
            let mut query = Vec::new();
            if let Some(status) = optional_str(&map, "status") {
                query.push(format!("status={status}"));
            }
            if let Some(source) = optional_str(&map, "source") {
                query.push(format!("source={source}"));
            }
            let path = if query.is_empty() {
                "/v1/tickets".to_string()
            } else {
                format!("/v1/tickets?{}", query.join("&"))
            };
            Ok(TeaAction::get(path))
        }
        "tea_get_ticket" => {
            let map = require_object(arguments)?;
            let ticket_id = require_str(map, "ticket_id")?;
            Ok(TeaAction::get(encode_ticket_path(&ticket_id, "")))
        }
        "tea_create_ticket" => {
            let map = require_object(arguments)?;
            let title = require_str(map, "title")?;
            let description = require_str(map, "description")?;
            let mut body = json!({ "title": title, "description": description });
            let object = body.as_object_mut().expect("create body is object");
            if let Some(priority) = optional_str(map, "priority") {
                object.insert("priority".to_string(), json!(priority));
            }
            if let Some(labels) = optional_labels(map, "labels")? {
                object.insert("labels".to_string(), json!(labels));
            }
            if let Some(policy) = optional_str(map, "approval_policy") {
                object.insert("approval_policy".to_string(), json!(policy));
            }
            Ok(TeaAction::post("/v1/tickets", body))
        }
        "tea_edit_ticket" => {
            let map = require_object(arguments)?;
            let ticket_id = require_str(map, "ticket_id")?;
            let mut body = serde_json::Map::new();
            if let Some(title) = optional_str(map, "title") {
                body.insert("title".to_string(), json!(title));
            }
            if let Some(description) = map.get("description").and_then(Value::as_str) {
                body.insert("description".to_string(), json!(description));
            }
            if let Some(priority) = optional_str(map, "priority") {
                body.insert("priority".to_string(), json!(priority));
            }
            if let Some(labels) = optional_labels(map, "labels")? {
                body.insert("labels".to_string(), json!(labels));
            }
            Ok(TeaAction::patch(
                encode_ticket_path(&ticket_id, ""),
                Value::Object(body),
            ))
        }
        "tea_comment_ticket" => {
            let map = require_object(arguments)?;
            let ticket_id = require_str(map, "ticket_id")?;
            let comment_body = require_str(map, "body")?;
            Ok(TeaAction::post(
                encode_ticket_path(&ticket_id, "/comments"),
                json!({ "body": comment_body }),
            ))
        }
        "tea_list_events" => {
            let map = require_object(arguments)?;
            let ticket_id = require_str(map, "ticket_id")?;
            Ok(TeaAction::get(encode_ticket_path(&ticket_id, "/events")))
        }
        "tea_list_comments" => {
            let map = require_object(arguments)?;
            let ticket_id = require_str(map, "ticket_id")?;
            Ok(TeaAction::get(encode_ticket_path(&ticket_id, "/comments")))
        }
        "tea_analyze_ticket" => simple_post(arguments, "/analyze"),
        "tea_decompose_ticket" => simple_post(arguments, "/decompose"),
        "tea_plan_ticket" => simple_post(arguments, "/plan"),
        "tea_approve_ticket" => simple_post(arguments, "/approve"),
        "tea_reject_ticket" => {
            let map = require_object(arguments)?;
            let ticket_id = require_str(map, "ticket_id")?;
            let reason = require_str(map, "reason")?;
            Ok(TeaAction::post(
                encode_ticket_path(&ticket_id, "/reject"),
                json!({ "reason": reason }),
            ))
        }
        "tea_run_ticket" => simple_post(arguments, "/run"),
        "tea_accept_ticket" => simple_post(arguments, "/accept"),
        "tea_close_ticket" => simple_post(arguments, "/close"),
        "tea_cancel_ticket" => simple_post(arguments, "/cancel"),
        "tea_export_ticket" => {
            let map = require_object(arguments)?;
            let ticket_id = require_str(map, "ticket_id")?;
            let format = optional_str(map, "format").unwrap_or_else(|| "json".to_string());
            match format.as_str() {
                "markdown" | "md" => Ok(TeaAction::get_text(encode_ticket_path(
                    &ticket_id,
                    "/export/markdown",
                ))),
                _ => Ok(TeaAction::get(encode_ticket_path(
                    &ticket_id,
                    "/export/json",
                ))),
            }
        }
        other => Err(McpError::UnknownTool(other.to_string())),
    }
}

fn simple_post(arguments: &Value, suffix: &str) -> Result<TeaAction, McpError> {
    let map = require_object(arguments)?;
    let ticket_id = require_str(map, "ticket_id")?;
    Ok(TeaAction::post(
        encode_ticket_path(&ticket_id, suffix),
        json!({}),
    ))
}

/// Build the JSON-RPC `initialize` result payload.
pub fn initialize_result() -> Value {
    json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
    })
}

/// Build the JSON-RPC `tools/list` result payload.
pub fn tools_list_result() -> Value {
    json!({ "tools": tool_catalog() })
}

/// Wrap a successful JSON-RPC response with the given id and result.
pub fn jsonrpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Wrap a JSON-RPC error response with the given id, code, and message.
pub fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

/// Standard JSON-RPC error code for an unknown method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// Standard JSON-RPC error code for invalid params.
pub const INVALID_PARAMS: i64 = -32602;
/// Application error code for a failed tool/upstream call.
pub const TOOL_EXECUTION_ERROR: i64 = -32000;

/// Build an MCP `tools/call` result body wrapping text content. When
/// `is_error` is true, the MCP client treats the content as a tool error.
pub fn tool_call_result(text: impl Into<String>, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text.into() }],
        "isError": is_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_unique_and_prefixed() {
        let names = tool_names();
        assert!(names.len() >= 18, "expected the full tool catalog");
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "tool names must be unique");
        for name in names {
            assert!(name.starts_with("tea_"), "tool {name} must be tea-prefixed");
        }
    }

    #[test]
    fn every_catalog_tool_resolves_with_valid_arguments() {
        // A representative valid argument set per tool; ensures the catalog and
        // resolver never drift apart.
        for tool in tool_catalog() {
            let args = match tool.name {
                "tea_status" | "tea_list_tickets" => json!({}),
                "tea_create_ticket" => json!({
                    "title": "A valid title",
                    "description": "A description long enough to pass.",
                }),
                "tea_edit_ticket" => json!({ "ticket_id": "abc", "title": "New" }),
                "tea_comment_ticket" => json!({ "ticket_id": "abc", "body": "hi" }),
                "tea_reject_ticket" => json!({ "ticket_id": "abc", "reason": "no" }),
                "tea_export_ticket" => json!({ "ticket_id": "abc", "format": "markdown" }),
                _ => json!({ "ticket_id": "abc" }),
            };
            let resolved = resolve_tool_call(tool.name, &args);
            assert!(resolved.is_ok(), "tool {} failed to resolve", tool.name);
        }
    }

    #[test]
    fn create_ticket_includes_optional_fields() {
        let action = resolve_tool_call(
            "tea_create_ticket",
            &json!({
                "title": "Smoke",
                "description": "Create a safe plan only.",
                "priority": "high",
                "labels": ["area:auth", "  ", "needs-triage"],
                "approval_policy": "plan_only",
            }),
        )
        .unwrap();
        assert_eq!(action.method, HttpMethod::Post);
        assert_eq!(action.path, "/v1/tickets");
        let body = action.body.unwrap();
        assert_eq!(body["title"], "Smoke");
        assert_eq!(body["priority"], "high");
        assert_eq!(body["approval_policy"], "plan_only");
        // Blank labels are dropped.
        assert_eq!(body["labels"], json!(["area:auth", "needs-triage"]));
    }

    #[test]
    fn create_ticket_requires_title_and_description() {
        assert_eq!(
            resolve_tool_call("tea_create_ticket", &json!({ "description": "only body" })),
            Err(McpError::MissingArgument("title"))
        );
        assert_eq!(
            resolve_tool_call("tea_create_ticket", &json!({ "title": "only title" })),
            Err(McpError::MissingArgument("description"))
        );
    }

    #[test]
    fn edit_ticket_sends_only_provided_fields() {
        let action = resolve_tool_call(
            "tea_edit_ticket",
            &json!({ "ticket_id": "t-1", "priority": "high" }),
        )
        .unwrap();
        assert_eq!(action.method, HttpMethod::Patch);
        assert_eq!(action.path, "/v1/tickets/t-1");
        let body = action.body.unwrap();
        assert_eq!(body, json!({ "priority": "high" }));
    }

    #[test]
    fn edit_ticket_allows_empty_description_but_not_blank_title() {
        // An explicitly empty description is a real edit (clears the body);
        // a blank title is treated as "not provided".
        let action = resolve_tool_call(
            "tea_edit_ticket",
            &json!({ "ticket_id": "t-1", "description": "", "title": "   " }),
        )
        .unwrap();
        let body = action.body.unwrap();
        assert_eq!(body, json!({ "description": "" }));
    }

    #[test]
    fn list_tickets_builds_query_from_filters() {
        let action = resolve_tool_call("tea_list_tickets", &json!({ "status": "open" })).unwrap();
        assert_eq!(action.path, "/v1/tickets?status=open");

        let action = resolve_tool_call(
            "tea_list_tickets",
            &json!({ "status": "running", "source": "hook" }),
        )
        .unwrap();
        assert_eq!(action.path, "/v1/tickets?status=running&source=hook");

        let action = resolve_tool_call("tea_list_tickets", &json!({})).unwrap();
        assert_eq!(action.path, "/v1/tickets");
    }

    #[test]
    fn lifecycle_tools_map_to_expected_endpoints() {
        let cases = [
            ("tea_analyze_ticket", "/v1/tickets/t-9/analyze"),
            ("tea_decompose_ticket", "/v1/tickets/t-9/decompose"),
            ("tea_plan_ticket", "/v1/tickets/t-9/plan"),
            ("tea_approve_ticket", "/v1/tickets/t-9/approve"),
            ("tea_run_ticket", "/v1/tickets/t-9/run"),
            ("tea_accept_ticket", "/v1/tickets/t-9/accept"),
            ("tea_close_ticket", "/v1/tickets/t-9/close"),
            ("tea_cancel_ticket", "/v1/tickets/t-9/cancel"),
        ];
        for (tool, expected_path) in cases {
            let action = resolve_tool_call(tool, &json!({ "ticket_id": "t-9" })).unwrap();
            assert_eq!(action.method, HttpMethod::Post, "{tool} method");
            assert_eq!(action.path, expected_path, "{tool} path");
            assert_eq!(action.body, Some(json!({})), "{tool} body");
        }
    }

    #[test]
    fn reject_requires_reason() {
        assert_eq!(
            resolve_tool_call("tea_reject_ticket", &json!({ "ticket_id": "t-1" })),
            Err(McpError::MissingArgument("reason"))
        );
        let action = resolve_tool_call(
            "tea_reject_ticket",
            &json!({ "ticket_id": "t-1", "reason": "needs evidence" }),
        )
        .unwrap();
        assert_eq!(action.path, "/v1/tickets/t-1/reject");
        assert_eq!(action.body, Some(json!({ "reason": "needs evidence" })));
    }

    #[test]
    fn export_defaults_to_json_and_supports_markdown() {
        let action =
            resolve_tool_call("tea_export_ticket", &json!({ "ticket_id": "t-1" })).unwrap();
        assert_eq!(action.path, "/v1/tickets/t-1/export/json");
        assert!(!action.expects_text);

        let action = resolve_tool_call(
            "tea_export_ticket",
            &json!({ "ticket_id": "t-1", "format": "markdown" }),
        )
        .unwrap();
        assert_eq!(action.path, "/v1/tickets/t-1/export/markdown");
        assert!(action.expects_text);
    }

    #[test]
    fn unknown_tool_is_reported() {
        assert_eq!(
            resolve_tool_call("tea_teleport", &json!({})),
            Err(McpError::UnknownTool("tea_teleport".to_string()))
        );
    }

    #[test]
    fn ticket_id_with_special_chars_is_encoded() {
        let action = resolve_tool_call("tea_get_ticket", &json!({ "ticket_id": "a/b c" })).unwrap();
        assert_eq!(action.path, "/v1/tickets/a%2Fb%20c");
    }

    #[test]
    fn missing_ticket_id_is_reported() {
        assert_eq!(
            resolve_tool_call("tea_get_ticket", &json!({})),
            Err(McpError::MissingArgument("ticket_id"))
        );
    }

    #[test]
    fn initialize_and_tools_list_shapes() {
        let init = initialize_result();
        assert_eq!(init["protocolVersion"], MCP_PROTOCOL_VERSION);
        assert_eq!(init["serverInfo"]["name"], SERVER_NAME);

        let listed = tools_list_result();
        let tools = listed["tools"].as_array().unwrap();
        assert_eq!(tools.len(), tool_catalog().len());
        assert_eq!(tools[0]["name"], "tea_status");
        assert!(tools[0]["inputSchema"]["type"] == "object");
    }

    #[test]
    fn jsonrpc_helpers_wrap_ids() {
        let ok = jsonrpc_result(json!(7), json!({ "ok": true }));
        assert_eq!(ok["jsonrpc"], "2.0");
        assert_eq!(ok["id"], 7);
        assert_eq!(ok["result"]["ok"], true);

        let err = jsonrpc_error(json!(8), METHOD_NOT_FOUND, "nope");
        assert_eq!(err["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(err["error"]["message"], "nope");
    }

    #[test]
    fn tool_call_result_marks_errors() {
        let ok = tool_call_result("done", false);
        assert_eq!(ok["isError"], false);
        assert_eq!(ok["content"][0]["text"], "done");
        let err = tool_call_result("boom", true);
        assert_eq!(err["isError"], true);
    }
}
