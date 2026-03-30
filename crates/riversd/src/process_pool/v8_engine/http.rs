//! HTTP verb callbacks, `do_http_request()`, header/response helpers,
//! `json_to_v8`/`v8_to_json`.

use std::collections::HashMap;

use super::super::types::*;
use super::task_locals::*;

/// Convert serde_json::Value -> V8 value via JSON.parse.
pub(super) fn json_to_v8<'s>(
    scope: &mut v8::HandleScope<'s>,
    value: &serde_json::Value,
) -> Result<v8::Local<'s, v8::Value>, TaskError> {
    let json_str = serde_json::to_string(value)
        .map_err(|e| TaskError::Internal(format!("json serialize: {e}")))?;
    let v8_str = v8::String::new(scope, &json_str)
        .ok_or_else(|| TaskError::Internal("failed to create V8 JSON string".into()))?;
    v8::json::parse(scope, v8_str.into())
        .ok_or_else(|| TaskError::Internal("V8 JSON.parse failed".into()))
}

/// Convert V8 value -> serde_json::Value via JSON.stringify.
pub(super) fn v8_to_json(
    scope: &mut v8::HandleScope,
    value: v8::Local<v8::Value>,
) -> Result<serde_json::Value, TaskError> {
    if value.is_undefined() || value.is_null() {
        return Ok(serde_json::Value::Null);
    }
    let json_str = v8::json::stringify(scope, value)
        .ok_or_else(|| TaskError::Internal("V8 JSON.stringify failed".into()))?;
    let rust_str = json_str.to_rust_string_lossy(scope);
    serde_json::from_str(&rust_str)
        .map_err(|e| TaskError::Internal(format!("parse JSON result: {e}")))
}

// ── Rivers.http Native Callbacks ────────────────────────────────

/// Helper: extract headers from a V8 object into a reqwest HeaderMap.
///
/// V8 callback helper -- short constant strings, unwrap is safe.
fn extract_headers_from_opts(
    scope: &mut v8::HandleScope,
    opts: v8::Local<v8::Value>,
) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    if opts.is_undefined() || opts.is_null() {
        return headers;
    }
    if let Ok(opts_obj) = v8::Local::<v8::Object>::try_from(opts) {
        let headers_key = v8::String::new(scope, "headers").unwrap();
        if let Some(h_val) = opts_obj.get(scope, headers_key.into()) {
            if let Ok(h_obj) = v8::Local::<v8::Object>::try_from(h_val) {
                if let Some(names) = h_obj.get_own_property_names(scope, Default::default()) {
                    for i in 0..names.length() {
                        if let Some(key) = names.get_index(scope, i) {
                            let key_str = key.to_rust_string_lossy(scope);
                            if let Some(val) = h_obj.get(scope, key) {
                                let val_str = val.to_rust_string_lossy(scope);
                                headers.insert(key_str, val_str);
                            }
                        }
                    }
                }
            }
        }
    }
    headers
}

/// Helper: convert an HTTP response (status + body) into a V8 value.
///
/// V8 callback helper -- short constant strings, unwrap is safe.
fn http_result_to_v8<'s>(
    scope: &mut v8::HandleScope<'s>,
    result: Result<serde_json::Value, String>,
) -> Option<v8::Local<'s, v8::Value>> {
    match result {
        Ok(json) => {
            let json_str = serde_json::to_string(&json).unwrap_or_else(|_| "{}".into());
            let v8_str = v8::String::new(scope, &json_str)?;
            v8::json::parse(scope, v8_str.into())
        }
        Err(e) => {
            let msg =
                v8::String::new(scope, &format!("Rivers.http request failed: {e}")).unwrap();
            let exception = v8::Exception::error(scope, msg);
            scope.throw_exception(exception);
            None
        }
    }
}

/// Extract host from URL for logging (avoids leaking query params / secrets).
fn extract_host(url: &str) -> &str {
    // Try to find host between :// and the next / or end
    if let Some(start) = url.find("://") {
        let after_scheme = &url[start + 3..];
        match after_scheme.find('/') {
            Some(end) => &url[start + 3..start + 3 + end],
            None => after_scheme,
        }
    } else {
        url
    }
}

/// Perform an HTTP request via the async bridge.
///
/// Per spec SS10.5: each call is logged at INFO with destination host and trace ID.
fn do_http_request(
    method: &str,
    url: &str,
    body: Option<&str>,
    headers: &HashMap<String, String>,
) -> Result<serde_json::Value, String> {
    // Log outbound request with host only (not full URL to avoid leaking secrets)
    let host = extract_host(url);
    let trace_id = TASK_TRACE_ID.with(|t| t.borrow().clone()).unwrap_or_default();
    tracing::info!(
        target: "rivers.http",
        method = %method,
        host = %host,
        trace_id = %trace_id,
        "outbound HTTP request"
    );

    let rt = get_rt_handle().map_err(|e| e.to_string())?;

    rt.block_on(async {
        let client = reqwest::Client::new();
        let mut builder = match method {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "DELETE" => client.delete(url),
            _ => return Err(format!("unsupported HTTP method: {method}")),
        };

        for (k, v) in headers {
            builder = builder.header(k.as_str(), v.as_str());
        }

        if let Some(body_str) = body {
            builder = builder
                .header("content-type", "application/json")
                .body(body_str.to_string());
        }

        let resp = builder.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let body_text = resp.text().await.map_err(|e| e.to_string())?;

        // Try to parse body as JSON, fall back to string
        let body_val: serde_json::Value = serde_json::from_str(&body_text)
            .unwrap_or(serde_json::Value::String(body_text));

        Ok(serde_json::json!({ "status": status, "body": body_val }))
    })
}

/// Rivers.http.get(url, opts?) callback.
/// V8 callback -- cannot return Result.
pub(super) fn rivers_http_get_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let headers = extract_headers_from_opts(scope, args.get(1));
    let result = do_http_request("GET", &url, None, &headers);
    if let Some(val) = http_result_to_v8(scope, result) {
        rv.set(val);
    }
}

/// Rivers.http.post(url, body, opts?) callback.
pub(super) fn rivers_http_post_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let body_val = args.get(1);
    let body_str = if body_val.is_undefined() || body_val.is_null() {
        None
    } else {
        v8::json::stringify(scope, body_val).map(|s| s.to_rust_string_lossy(scope))
    };
    let headers = extract_headers_from_opts(scope, args.get(2));
    let result = do_http_request("POST", &url, body_str.as_deref(), &headers);
    if let Some(val) = http_result_to_v8(scope, result) {
        rv.set(val);
    }
}

/// Rivers.http.put(url, body, opts?) callback.
pub(super) fn rivers_http_put_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let body_val = args.get(1);
    let body_str = if body_val.is_undefined() || body_val.is_null() {
        None
    } else {
        v8::json::stringify(scope, body_val).map(|s| s.to_rust_string_lossy(scope))
    };
    let headers = extract_headers_from_opts(scope, args.get(2));
    let result = do_http_request("PUT", &url, body_str.as_deref(), &headers);
    if let Some(val) = http_result_to_v8(scope, result) {
        rv.set(val);
    }
}

/// Rivers.http.del(url, opts?) callback.
pub(super) fn rivers_http_del_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    let url = args.get(0).to_rust_string_lossy(scope);
    let headers = extract_headers_from_opts(scope, args.get(1));
    let result = do_http_request("DELETE", &url, None, &headers);
    if let Some(val) = http_result_to_v8(scope, result) {
        rv.set(val);
    }
}
