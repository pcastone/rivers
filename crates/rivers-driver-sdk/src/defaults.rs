//! Shared driver defaults — timeout constants, row/byte caps, and option readers.
//!
//! All drivers should use these constants rather than hard-coding their own
//! values, so that operator-level tuning is consistent across plugins.

use crate::traits::ConnectionParams;

// ── Timeout constants ────────────────────────────────────────────────────────

/// Default TCP connect timeout for all driver HTTP/TCP connections (seconds).
pub const DEFAULT_CONNECT_TIMEOUT_SECS: u64 = 10;

/// Default request/read timeout for all driver HTTP requests (seconds).
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

// ── Result size constants ────────────────────────────────────────────────────

/// Default maximum rows returned by any single driver query.
pub const DEFAULT_MAX_ROWS: usize = 10_000;

/// Default maximum response body size for any single driver HTTP response (bytes).
pub const DEFAULT_MAX_RESPONSE_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB

// ── Option readers ───────────────────────────────────────────────────────────

/// Read the connect timeout (seconds) from `params.options`, falling back to
/// [`DEFAULT_CONNECT_TIMEOUT_SECS`].
///
/// Option key: `"connect_timeout_secs"`.
pub fn read_connect_timeout(params: &ConnectionParams) -> u64 {
    params
        .options
        .get("connect_timeout_secs")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CONNECT_TIMEOUT_SECS)
}

/// Read the request timeout (seconds) from `params.options`, falling back to
/// [`DEFAULT_REQUEST_TIMEOUT_SECS`].
///
/// Option key: `"request_timeout_secs"`.
pub fn read_request_timeout(params: &ConnectionParams) -> u64 {
    params
        .options
        .get("request_timeout_secs")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS)
}

/// Read the max rows cap from `params.options`, falling back to [`DEFAULT_MAX_ROWS`].
///
/// Option key: `"max_rows"`.
pub fn read_max_rows(params: &ConnectionParams) -> usize {
    params
        .options
        .get("max_rows")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_ROWS)
}

// ── URL encoding ─────────────────────────────────────────────────────────────

/// Percent-encode a URL path-segment or query-parameter component.
///
/// Unreserved characters (`A–Z`, `a–z`, `0–9`, `-`, `_`, `.`, `~`) pass
/// through unchanged; every other byte is encoded as `%XX` (uppercase hex).
/// This matches RFC 3986 §2.3 unreserved characters and is safe for use in
/// AMQP virtual-host paths, InfluxDB org/bucket names, and similar contexts.
pub fn url_encode_path_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn params_with(key: &str, val: &str) -> ConnectionParams {
        let mut options = HashMap::new();
        options.insert(key.to_string(), val.to_string());
        ConnectionParams {
            host: "localhost".into(),
            port: 0,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            options,
        }
    }

    fn empty_params() -> ConnectionParams {
        ConnectionParams {
            host: "localhost".into(),
            port: 0,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            options: HashMap::new(),
        }
    }

    // ── read_connect_timeout ─────────────────────────────────────────────────

    #[test]
    fn read_connect_timeout_default() {
        let p = empty_params();
        assert_eq!(read_connect_timeout(&p), DEFAULT_CONNECT_TIMEOUT_SECS);
    }

    #[test]
    fn read_connect_timeout_from_option() {
        let p = params_with("connect_timeout_secs", "5");
        assert_eq!(read_connect_timeout(&p), 5);
    }

    #[test]
    fn read_connect_timeout_invalid_falls_back() {
        let p = params_with("connect_timeout_secs", "not-a-number");
        assert_eq!(read_connect_timeout(&p), DEFAULT_CONNECT_TIMEOUT_SECS);
    }

    // ── read_request_timeout ─────────────────────────────────────────────────

    #[test]
    fn read_request_timeout_default() {
        let p = empty_params();
        assert_eq!(read_request_timeout(&p), DEFAULT_REQUEST_TIMEOUT_SECS);
    }

    #[test]
    fn read_request_timeout_from_option() {
        let p = params_with("request_timeout_secs", "60");
        assert_eq!(read_request_timeout(&p), 60);
    }

    // ── read_max_rows ────────────────────────────────────────────────────────

    #[test]
    fn read_max_rows_default() {
        let p = empty_params();
        assert_eq!(read_max_rows(&p), DEFAULT_MAX_ROWS);
    }

    #[test]
    fn read_max_rows_from_option() {
        let p = params_with("max_rows", "500");
        assert_eq!(read_max_rows(&p), 500);
    }

    #[test]
    fn read_max_rows_invalid_falls_back() {
        let p = params_with("max_rows", "abc");
        assert_eq!(read_max_rows(&p), DEFAULT_MAX_ROWS);
    }

    // ── url_encode_path_segment ──────────────────────────────────────────────

    #[test]
    fn url_encode_unreserved_chars_pass_through() {
        assert_eq!(url_encode_path_segment("AZaz09-_.~"), "AZaz09-_.~");
    }

    #[test]
    fn url_encode_space() {
        assert_eq!(url_encode_path_segment("hello world"), "hello%20world");
    }

    #[test]
    fn url_encode_at_sign() {
        assert_eq!(url_encode_path_segment("user@host"), "user%40host");
    }

    #[test]
    fn url_encode_slash() {
        assert_eq!(url_encode_path_segment("a/b"), "a%2Fb");
    }

    #[test]
    fn url_encode_special_chars() {
        assert_eq!(url_encode_path_segment("p@ss:w0rd!"), "p%40ss%3Aw0rd%21");
    }

    #[test]
    fn url_encode_empty_string() {
        assert_eq!(url_encode_path_segment(""), "");
    }

    #[test]
    fn url_encode_colon() {
        assert_eq!(url_encode_path_segment("a:b"), "a%3Ab");
    }
}
