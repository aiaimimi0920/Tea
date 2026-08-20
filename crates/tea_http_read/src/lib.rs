//! Shared security-relevant HTTP helpers for Tea binaries: bounded response
//! readers, sanitized error-body previews, and URL component percent-encoding.
//!
//! These used to be copy-pasted into every binary and drifted; each caller's
//! contract tests assert the exact error-message text produced here, so keep
//! the wording stable.

#![forbid(unsafe_code)]

/// Maximum number of characters kept in an [`error_body_preview`].
pub const MAX_ERROR_PREVIEW_CHARS: usize = 512;

/// Bound and sanitize a response body before interpolating it into an error
/// message: single line, control characters stripped (including the Unicode
/// line separators U+2028/U+2029), capped at [`MAX_ERROR_PREVIEW_CHARS`]
/// characters with a trailing `...` when truncated.
pub fn error_body_preview(body: &str) -> String {
    let mut characters = body.chars();
    let mut preview = String::with_capacity(MAX_ERROR_PREVIEW_CHARS + 3);
    for character in characters.by_ref().take(MAX_ERROR_PREVIEW_CHARS) {
        match character {
            '\r' | '\n' | '\t' | '\u{2028}' | '\u{2029}' => preview.push(' '),
            character if character.is_control() => {}
            character => preview.push(character),
        }
    }
    if characters.next().is_some() {
        preview.push_str("...");
    }
    preview
}

/// Percent-encode a URL path segment or query value. Everything except
/// RFC 3986 unreserved characters (ALPHA / DIGIT / `-` / `.` / `_` / `~`) is
/// encoded as uppercase `%XX` byte escapes.
pub fn percent_encode_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

#[cfg(feature = "http")]
mod http {
    use thiserror::Error;

    /// Error from [`read_response_bytes_limited`]. Carries no caller context so
    /// wrappers (e.g. tea_loom/tea_brain) can phrase their own messages.
    #[derive(Debug, Error)]
    pub enum ReadBytesLimitedError {
        #[error("response body exceeded the {max_bytes}-byte limit")]
        BodyTooLarge { max_bytes: usize },
        #[error("response body length overflowed")]
        LengthOverflow,
        #[error(transparent)]
        Read(#[from] reqwest::Error),
    }

    /// Error from [`read_response_text_limited`]. The `Display` strings are
    /// contract-tested by every Tea binary; do not reword them.
    #[derive(Debug, Error)]
    pub enum ReadLimitedError {
        #[error("{context} response body exceeded the {max_bytes}-byte limit")]
        BodyTooLarge { context: String, max_bytes: usize },
        #[error("{context} response body length overflowed")]
        LengthOverflow { context: String },
        #[error("{context} response body was not valid UTF-8")]
        InvalidUtf8 {
            context: String,
            #[source]
            source: std::string::FromUtf8Error,
        },
        #[error(transparent)]
        Read(#[from] reqwest::Error),
    }

    /// Stream a response body into memory, rejecting bodies larger than
    /// `max_bytes` (both via the declared `Content-Length` and while
    /// streaming, so chunked responses cannot bypass the limit).
    pub async fn read_response_bytes_limited(
        mut response: reqwest::Response,
        max_bytes: usize,
    ) -> Result<(reqwest::StatusCode, Vec<u8>), ReadBytesLimitedError> {
        let status = response.status();
        if response
            .content_length()
            .is_some_and(|length| length > max_bytes as u64)
        {
            return Err(ReadBytesLimitedError::BodyTooLarge { max_bytes });
        }

        let mut body = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or_default()
                .min(max_bytes as u64) as usize,
        );
        while let Some(chunk) = response.chunk().await? {
            let next_length = body
                .len()
                .checked_add(chunk.len())
                .ok_or(ReadBytesLimitedError::LengthOverflow)?;
            if next_length > max_bytes {
                return Err(ReadBytesLimitedError::BodyTooLarge { max_bytes });
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, body))
    }

    /// [`read_response_bytes_limited`] plus UTF-8 decoding, with `context`
    /// (e.g. `"Tea API"`) baked into every error message.
    pub async fn read_response_text_limited(
        response: reqwest::Response,
        max_bytes: usize,
        context: &str,
    ) -> Result<(reqwest::StatusCode, String), ReadLimitedError> {
        let (status, body) = read_response_bytes_limited(response, max_bytes)
            .await
            .map_err(|error| match error {
                ReadBytesLimitedError::BodyTooLarge { max_bytes } => {
                    ReadLimitedError::BodyTooLarge {
                        context: context.to_string(),
                        max_bytes,
                    }
                }
                ReadBytesLimitedError::LengthOverflow => ReadLimitedError::LengthOverflow {
                    context: context.to_string(),
                },
                ReadBytesLimitedError::Read(source) => ReadLimitedError::Read(source),
            })?;
        let text = String::from_utf8(body).map_err(|source| ReadLimitedError::InvalidUtf8 {
            context: context.to_string(),
            source,
        })?;
        Ok((status, text))
    }
}

#[cfg(feature = "http")]
pub use http::{
    read_response_bytes_limited, read_response_text_limited, ReadBytesLimitedError,
    ReadLimitedError,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_is_single_line_and_capped() {
        let body = format!("first\r\nsecond\tthird{}", "x".repeat(600));
        let preview = error_body_preview(&body);

        assert!(preview.starts_with("first  second third"));
        assert!(preview.ends_with("..."));
        assert!(preview.chars().count() <= MAX_ERROR_PREVIEW_CHARS + 3);
        assert!(!preview
            .chars()
            .any(|character| matches!(character, '\n' | '\r' | '\t')));
    }

    #[test]
    fn preview_strips_control_chars_and_unicode_line_separators() {
        assert_eq!(
            error_body_preview("a\u{1b}b\u{2028}c\u{2029}d\u{0}e"),
            "ab c de"
        );
    }

    #[test]
    fn preview_without_truncation_has_no_ellipsis() {
        assert_eq!(error_body_preview("short body"), "short body");
        let exactly_max = "y".repeat(MAX_ERROR_PREVIEW_CHARS);
        assert_eq!(error_body_preview(&exactly_max), exactly_max);
        let one_over = "y".repeat(MAX_ERROR_PREVIEW_CHARS + 1);
        assert_eq!(error_body_preview(&one_over), format!("{exactly_max}..."));
    }

    #[test]
    fn percent_encoder_keeps_unreserved_and_encodes_the_rest() {
        assert_eq!(percent_encode_component("AZaz09-_.~"), "AZaz09-_.~");
        assert_eq!(
            percent_encode_component("a b/c?d&e=f"),
            "a%20b%2Fc%3Fd%26e%3Df"
        );
        assert_eq!(
            percent_encode_component("中文/é"),
            "%E4%B8%AD%E6%96%87%2F%C3%A9"
        );
        assert_eq!(percent_encode_component(""), "");
        assert_eq!(percent_encode_component("%"), "%25");
        assert_eq!(percent_encode_component("a+b"), "a%2Bb");
    }
}

#[cfg(all(test, feature = "http"))]
mod http_tests {
    use super::*;

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

    #[tokio::test]
    async fn text_reader_rejects_oversized_content_length() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 9\r\n",
            "Connection: close\r\n\r\n",
            "123456789"
        )
        .to_string();
        let (url, server) = spawn_raw_http_server(response);
        let response = reqwest::Client::new().get(url).send().await.unwrap();

        let error = read_response_text_limited(response, 8, "test API")
            .await
            .unwrap_err();

        server.join().unwrap();
        assert_eq!(
            error.to_string(),
            "test API response body exceeded the 8-byte limit"
        );
    }

    #[tokio::test]
    async fn text_reader_rejects_oversized_chunked_body() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Connection: close\r\n\r\n",
            "5\r\n12345\r\n",
            "5\r\n67890\r\n",
            "0\r\n\r\n"
        )
        .to_string();
        let (url, server) = spawn_raw_http_server(response);
        let response = reqwest::Client::new().get(url).send().await.unwrap();

        let error = read_response_text_limited(response, 8, "test API")
            .await
            .unwrap_err();

        server.join().unwrap();
        assert_eq!(
            error.to_string(),
            "test API response body exceeded the 8-byte limit"
        );
    }

    #[tokio::test]
    async fn text_reader_rejects_invalid_utf8() {
        let headers = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 2\r\n",
            "Connection: close\r\n\r\n"
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            use std::io::{Read as _, Write as _};

            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).unwrap();
            stream.write_all(headers.as_bytes()).unwrap();
            stream.write_all(&[0xff, 0xfe]).unwrap();
            stream.flush().unwrap();
        });

        let response = reqwest::Client::new()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap();
        let error = read_response_text_limited(response, 8, "test API")
            .await
            .unwrap_err();

        server.join().unwrap();
        assert_eq!(
            error.to_string(),
            "test API response body was not valid UTF-8"
        );
    }

    #[tokio::test]
    async fn readers_return_status_and_body_within_limits() {
        let response = concat!(
            "HTTP/1.1 404 Not Found\r\n",
            "Content-Length: 5\r\n",
            "Connection: close\r\n\r\n",
            "hello"
        )
        .to_string();
        let (url, server) = spawn_raw_http_server(response);
        let response = reqwest::Client::new().get(url).send().await.unwrap();

        let (status, text) = read_response_text_limited(response, 8, "test API")
            .await
            .unwrap();

        server.join().unwrap();
        assert_eq!(status, reqwest::StatusCode::NOT_FOUND);
        assert_eq!(text, "hello");
    }

    #[tokio::test]
    async fn bytes_reader_reports_limit_without_context() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Length: 9\r\n",
            "Connection: close\r\n\r\n",
            "123456789"
        )
        .to_string();
        let (url, server) = spawn_raw_http_server(response);
        let response = reqwest::Client::new().get(url).send().await.unwrap();

        let error = read_response_bytes_limited(response, 8).await.unwrap_err();

        server.join().unwrap();
        assert!(matches!(
            error,
            ReadBytesLimitedError::BodyTooLarge { max_bytes: 8 }
        ));
        assert_eq!(error.to_string(), "response body exceeded the 8-byte limit");
    }
}
