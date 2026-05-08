//! Static response-header injection for views (CB-P1.11).
//!
//! Configured via `[api.views.*.response_headers]`. Validation rejects
//! malformed names, non-printable values, and a small reserved set
//! (`Content-Type`, `Content-Length`, `Transfer-Encoding`,
//! `Mcp-Session-Id`) at bundle-load time, so by the time
//! [`apply_static_response_headers`] runs every entry is well-formed.
//!
//! Precedence: handler-set headers win. The framework only inserts a
//! configured header when the response does not already carry it.

use std::collections::HashMap;

use axum::http::{HeaderName, HeaderValue};
use axum::response::Response;

/// Append per-view static headers to `response`, preserving any
/// handler-set headers of the same name.
///
/// `config_headers` is `None` when the view has no `response_headers`
/// table — in that case the function is a no-op. Keys / values that
/// somehow fail axum's parser at runtime (e.g. survived structural
/// validation but trip a tighter check) are skipped with a WARN log
/// rather than dropping the response — failure to set a deprecation
/// header should not turn a 200 into a 500.
pub fn apply_static_response_headers(
    response: &mut Response,
    config_headers: Option<&HashMap<String, String>>,
) {
    let Some(map) = config_headers else { return };
    let headers = response.headers_mut();
    for (name, value) in map {
        let parsed_name = match HeaderName::try_from(name.as_str()) {
            Ok(n) => n,
            Err(err) => {
                tracing::warn!(
                    header = %name,
                    error = %err,
                    "view response_headers: invalid header name at runtime — skipped",
                );
                continue;
            }
        };
        if headers.contains_key(&parsed_name) {
            // Handler override wins.
            continue;
        }
        let parsed_value = match HeaderValue::try_from(value.as_str()) {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(
                    header = %name,
                    error = %err,
                    "view response_headers: invalid header value at runtime — skipped",
                );
                continue;
            }
        };
        headers.insert(parsed_name, parsed_value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    fn make_response_with_headers(initial: &[(&str, &str)]) -> Response {
        let mut builder = Response::builder().status(200);
        for (k, v) in initial {
            builder = builder.header(*k, *v);
        }
        builder.body(Body::empty()).unwrap()
    }

    /// CB-P1.11: configured headers are applied to a response that does
    /// not already carry them.
    #[test]
    fn applies_configured_headers_when_absent() {
        let mut resp = make_response_with_headers(&[]);
        let mut cfg = HashMap::new();
        cfg.insert("Deprecation".into(), "true".into());
        cfg.insert("Sunset".into(), "Wed, 31 Dec 2026 23:59:59 GMT".into());
        apply_static_response_headers(&mut resp, Some(&cfg));
        assert_eq!(resp.headers().get("deprecation").unwrap(), "true");
        assert_eq!(
            resp.headers().get("sunset").unwrap(),
            "Wed, 31 Dec 2026 23:59:59 GMT",
        );
    }

    /// CB-P1.11: handler-set headers win when both sides set the same name.
    #[test]
    fn handler_override_wins() {
        let mut resp = make_response_with_headers(&[("cache-control", "no-store")]);
        let mut cfg = HashMap::new();
        cfg.insert("Cache-Control".into(), "max-age=60".into());
        apply_static_response_headers(&mut resp, Some(&cfg));
        assert_eq!(
            resp.headers().get("cache-control").unwrap(),
            "no-store",
            "handler-set value must not be overwritten by config",
        );
        // Exactly one cache-control header should be present.
        assert_eq!(resp.headers().get_all("cache-control").iter().count(), 1);
    }

    /// CB-P1.11: missing config table is a safe no-op.
    #[test]
    fn no_op_when_config_is_none() {
        let mut resp = make_response_with_headers(&[("x-existing", "1")]);
        apply_static_response_headers(&mut resp, None);
        assert_eq!(resp.headers().get("x-existing").unwrap(), "1");
        assert_eq!(resp.headers().len(), 1);
    }

    /// Defense-in-depth: even though structural validation rejects them,
    /// the runtime helper must not panic on malformed entries that somehow
    /// reach it — it skips and logs.
    #[test]
    fn malformed_entries_are_skipped_not_panicking() {
        let mut resp = make_response_with_headers(&[]);
        let mut cfg = HashMap::new();
        cfg.insert("Bad Name With Spaces".into(), "ok".into());
        cfg.insert("Valid".into(), "value\nwith-newline".into());
        cfg.insert("X-Ok".into(), "ok".into());
        apply_static_response_headers(&mut resp, Some(&cfg));
        // Only the valid entry should make it through.
        assert_eq!(resp.headers().get("x-ok").unwrap(), "ok");
        assert!(resp.headers().get("bad name with spaces").is_none());
        assert!(resp.headers().get("valid").is_none());
    }
}
