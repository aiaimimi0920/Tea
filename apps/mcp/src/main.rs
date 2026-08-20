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
#[cfg(test)]
use tea_http_read::MAX_ERROR_PREVIEW_CHARS;
use tea_http_read::{error_body_preview, read_response_text_limited};
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
/// JSON-RPC invalid-request code per the spec.
const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC method-not-found code per the spec.
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC invalid-params code per the spec.
const INVALID_PARAMS: i64 = -32602;
const MAX_MCP_INPUT_LINE_BYTES: usize = 2 * 1024 * 1024;
const MAX_TEA_API_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
enum BoundedLine {
    Line(String),
    TooLong,
    InvalidUtf8,
    Eof,
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> std::io::Result<BoundedLine> {
    let mut line = Vec::with_capacity(max_bytes.min(8 * 1024));
    let mut too_long = false;

    loop {
        let buffer = reader.fill_buf()?;
        if buffer.is_empty() {
            if too_long {
                return Ok(BoundedLine::TooLong);
            }
            if line.is_empty() {
                return Ok(BoundedLine::Eof);
            }
            return Ok(match String::from_utf8(line) {
                Ok(line) => BoundedLine::Line(line),
                Err(_) => BoundedLine::InvalidUtf8,
            });
        }

        let newline = buffer.iter().position(|byte| *byte == b'\n');
        let segment_length = newline.unwrap_or(buffer.len());
        if !too_long {
            let remaining = max_bytes.saturating_sub(line.len());
            if segment_length > remaining {
                too_long = true;
            } else {
                line.extend_from_slice(&buffer[..segment_length]);
            }
        }

        let consumed = segment_length + usize::from(newline.is_some());
        reader.consume(consumed);
        if newline.is_some() {
            if too_long {
                return Ok(BoundedLine::TooLong);
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(match String::from_utf8(line) {
                Ok(line) => BoundedLine::Line(line),
                Err(_) => BoundedLine::InvalidUtf8,
            });
        }
    }
}

fn write_jsonrpc_response<W: Write>(writer: &mut W, response: &Value) -> anyhow::Result<()> {
    let serialized = serde_json::to_string(response)?;
    writer.write_all(serialized.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let client = TeaHttpClient::new(cli.server_url, cli.auth_token);

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    // Read one bounded JSON-RPC message per line. Oversized or invalid UTF-8
    // messages are drained through their newline before the next request.
    let mut input = stdin.lock();
    let mut line_index = 0_usize;
    loop {
        let line = match read_bounded_line(&mut input, MAX_MCP_INPUT_LINE_BYTES)? {
            BoundedLine::Line(line) => line,
            BoundedLine::TooLong => {
                write_jsonrpc_response(
                    &mut stdout,
                    &jsonrpc_error(
                        Value::Null,
                        PARSE_ERROR,
                        format!(
                            "JSON-RPC message exceeded the {MAX_MCP_INPUT_LINE_BYTES}-byte limit"
                        ),
                    ),
                )?;
                line_index += 1;
                continue;
            }
            BoundedLine::InvalidUtf8 => {
                write_jsonrpc_response(
                    &mut stdout,
                    &jsonrpc_error(
                        Value::Null,
                        PARSE_ERROR,
                        "JSON-RPC message was not valid UTF-8",
                    ),
                )?;
                line_index += 1;
                continue;
            }
            BoundedLine::Eof => break,
        };
        let trimmed = normalize_input_line(&line, line_index == 0);
        line_index += 1;
        if trimmed.is_empty() {
            continue;
        }

        let Some(response) = handle_line(trimmed, &client).await else {
            // Notifications (no id) produce no response.
            continue;
        };

        write_jsonrpc_response(&mut stdout, &response)?;
    }

    Ok(())
}

fn normalize_input_line(line: &str, first_line: bool) -> &str {
    let line = if first_line {
        line.strip_prefix('\u{feff}').unwrap_or(line)
    } else {
        line
    };
    line.trim()
}

/// Handle a single JSON-RPC line, returning `Some(response)` for requests and
/// `None` for valid notifications.
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

    let Some(message) = message.as_object() else {
        return Some(jsonrpc_error(
            Value::Null,
            INVALID_REQUEST,
            "JSON-RPC request must be an object",
        ));
    };
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(jsonrpc_error(
            Value::Null,
            INVALID_REQUEST,
            "JSON-RPC request must declare jsonrpc 2.0",
        ));
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(jsonrpc_error(
            Value::Null,
            INVALID_REQUEST,
            "JSON-RPC request method must be a string",
        ));
    };
    let id = match message.get("id") {
        None | Some(Value::Null) => None,
        Some(id @ (Value::String(_) | Value::Number(_))) => Some(id.clone()),
        Some(_) => {
            return Some(jsonrpc_error(
                Value::Null,
                INVALID_REQUEST,
                "JSON-RPC request id must be a string, number, or null",
            ));
        }
    };

    // Notifications (no id) are acknowledged silently.
    let id = id?;

    match method {
        "initialize" => Some(jsonrpc_result(id, initialize_result())),
        "tools/list" => Some(jsonrpc_result(id, tools_list_result())),
        "tools/call" => Some(handle_tools_call(id, message, client).await),
        "ping" => Some(jsonrpc_result(id, json!({}))),
        other => Some(jsonrpc_error(
            id,
            METHOD_NOT_FOUND,
            format!("method not supported: {other}"),
        )),
    }
}

/// Shared empty-object default for absent `params`/`arguments`, so tool calls
/// borrow from the incoming message instead of deep-cloning it.
fn empty_object() -> &'static Value {
    static EMPTY: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    EMPTY.get_or_init(|| Value::Object(serde_json::Map::new()))
}

async fn handle_tools_call(
    id: Value,
    message: &serde_json::Map<String, Value>,
    client: &TeaHttpClient,
) -> Value {
    let params = message.get("params").unwrap_or_else(|| empty_object());
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    // Absent "arguments" defaults to {}; an explicit "arguments": null is kept
    // as-is so it still fails tool-argument validation.
    let arguments = params.get("arguments").unwrap_or_else(|| empty_object());

    let action = match resolve_tool_call(name, arguments) {
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
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(300))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("build Tea MCP HTTP client"),
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
        if let Some(key) = &action.idempotency_key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(body) = &action.body {
            request = request.json(body);
        }

        let response = request.send().await?;
        let (status, body) =
            read_response_text_limited(response, MAX_TEA_API_RESPONSE_BYTES, "Tea API").await?;
        if !status.is_success() {
            anyhow::bail!("Tea API returned {status}: {}", error_body_preview(&body));
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

#[cfg(test)]
mod tests {
    use super::{
        error_body_preview, handle_line, normalize_input_line, read_bounded_line,
        read_response_text_limited, BoundedLine, TeaHttpClient, INVALID_REQUEST,
        MAX_ERROR_PREVIEW_CHARS,
    };
    use serde_json::Value;
    use std::io::Cursor;
    use tea_mcp::{HttpMethod, TeaAction};

    fn spawn_raw_http_server(response: String) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};

            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });
        (format!("http://{address}"), server)
    }

    fn spawn_capturing_http_server(
        response: String,
    ) -> (
        String,
        std::sync::mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};

            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let read = stream.read(&mut request).unwrap();
            sender
                .send(String::from_utf8_lossy(&request[..read]).to_string())
                .unwrap();
            stream.write_all(response.as_bytes()).unwrap();
            stream.flush().unwrap();
        });
        (format!("http://{address}"), receiver, server)
    }

    #[tokio::test]
    async fn mcp_response_reader_rejects_oversized_content_length() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 9\r\n",
            "Connection: close\r\n\r\n",
            "123456789"
        )
        .to_string();
        let (url, server) = spawn_raw_http_server(response);
        let response = reqwest::Client::new().get(url).send().await.unwrap();

        let error = read_response_text_limited(response, 8, "Tea API")
            .await
            .unwrap_err();

        server.join().unwrap();
        assert!(error.to_string().contains("8-byte limit"));
    }

    #[test]
    fn mcp_error_preview_is_bounded_and_single_line() {
        let body = format!("first\nsecond\t{}", "x".repeat(600));
        let preview = error_body_preview(&body);

        assert!(!preview
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '\t')));
        assert!(preview.ends_with("..."));
        assert!(preview.chars().count() <= MAX_ERROR_PREVIEW_CHARS + 3);
    }

    #[tokio::test]
    async fn mcp_http_errors_do_not_echo_unbounded_multiline_bodies() {
        let body = format!("first\nsecond\t{}", "x".repeat(600));
        let response = format!(
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (url, server) = spawn_raw_http_server(response);
        let client = TeaHttpClient::new(url, "test-token".to_string());
        let action = TeaAction {
            method: HttpMethod::Get,
            path: "/v1/status".to_string(),
            body: None,
            idempotency_key: None,
            expects_text: false,
        };

        let message = client.execute(&action).await.unwrap_err().to_string();

        server.join().unwrap();
        assert!(message.contains("500 Internal Server Error"));
        assert!(!message
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '\t')));
        assert!(message.ends_with("..."));
        assert!(message.chars().count() < 600);
    }

    #[tokio::test]
    async fn mcp_http_client_sends_idempotency_key_header() {
        let response =
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_string();
        let (url, request, server) = spawn_capturing_http_server(response);
        let client = TeaHttpClient::new(url, "test-token".to_string());
        let action = TeaAction {
            method: HttpMethod::Post,
            path: "/v1/tickets".to_string(),
            body: Some(serde_json::json!({
                "title": "MCP header test",
                "description": "Verify idempotency header propagation."
            })),
            idempotency_key: Some("mcp-idempotency-1".to_string()),
            expects_text: false,
        };

        client.execute(&action).await.unwrap();

        server.join().unwrap();
        let request = request.recv().unwrap().to_ascii_lowercase();
        assert!(request.contains("idempotency-key: mcp-idempotency-1\r\n"));
        assert!(request.contains("authorization: bearer test-token\r\n"));
    }

    #[test]
    fn input_framing_strips_a_bom_only_from_the_first_line() {
        let message = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;

        assert_eq!(
            normalize_input_line(&format!("\u{feff}{message}"), true),
            message
        );
        assert_eq!(
            normalize_input_line(&format!("\u{feff}{message}"), false),
            format!("\u{feff}{message}")
        );
    }

    #[test]
    fn bounded_input_accepts_a_line_at_the_limit() {
        let mut input = Cursor::new(format!("{}\n", "x".repeat(8)).into_bytes());

        assert_eq!(
            read_bounded_line(&mut input, 8).unwrap(),
            BoundedLine::Line("x".repeat(8))
        );
        assert_eq!(read_bounded_line(&mut input, 8).unwrap(), BoundedLine::Eof);
    }

    #[test]
    fn bounded_input_drains_an_oversized_line_before_the_next_message() {
        let mut input = Cursor::new(b"123456789\n{\"jsonrpc\":\"2.0\"}\r\n".to_vec());

        assert_eq!(
            read_bounded_line(&mut input, 8).unwrap(),
            BoundedLine::TooLong
        );
        assert_eq!(
            read_bounded_line(&mut input, 64).unwrap(),
            BoundedLine::Line(r#"{"jsonrpc":"2.0"}"#.to_string())
        );
    }

    #[test]
    fn bounded_input_rejects_an_oversized_final_line_without_newline() {
        let mut input = Cursor::new(b"123456789".to_vec());

        assert_eq!(
            read_bounded_line(&mut input, 8).unwrap(),
            BoundedLine::TooLong
        );
        assert_eq!(read_bounded_line(&mut input, 8).unwrap(), BoundedLine::Eof);
    }

    #[test]
    fn bounded_input_reports_invalid_utf8_without_losing_the_next_line() {
        let mut input = Cursor::new(vec![0xff, b'\n', b'{', b'}', b'\n']);

        assert_eq!(
            read_bounded_line(&mut input, 8).unwrap(),
            BoundedLine::InvalidUtf8
        );
        assert_eq!(
            read_bounded_line(&mut input, 8).unwrap(),
            BoundedLine::Line("{}".to_string())
        );
    }

    #[tokio::test]
    async fn null_id_is_treated_as_a_notification() {
        let client = TeaHttpClient::new(
            "http://127.0.0.1:48910".to_string(),
            "dev-token".to_string(),
        );
        assert!(
            handle_line(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#, &client)
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn malformed_jsonrpc_shapes_return_invalid_request() {
        let client = TeaHttpClient::new(
            "http://127.0.0.1:48910".to_string(),
            "dev-token".to_string(),
        );
        for message in [
            "[]",
            "null",
            "1",
            r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":1}"#,
            r#"{"jsonrpc":"2.0","id":true,"method":"ping"}"#,
        ] {
            let response = handle_line(message, &client)
                .await
                .expect("invalid request must receive an error");
            assert_eq!(response["id"], Value::Null, "message={message}");
            assert_eq!(
                response["error"]["code"], INVALID_REQUEST,
                "message={message}"
            );
        }
    }
}
