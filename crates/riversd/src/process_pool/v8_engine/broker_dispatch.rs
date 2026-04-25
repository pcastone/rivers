//! V8 bridge for `ctx.datasource("<broker>").publish(...)` — BR-2026-04-23.
//!
//! Mirrors `direct_dispatch.rs` but routes through `MessageBrokerDriver`
//! + `BrokerProducer::publish` instead of `DatabaseDriver::execute`.
//!
//! Wiring:
//!   1. `inject_broker_publish_callback` adds `Rivers.__brokerPublish` to the V8
//!      global at context-setup time.
//!   2. `bootstrap_broker_proxies` installs one typed proxy per broker
//!      datasource into `__rivers_direct_proxies` (shared registry with the
//!      filesystem proxies) so `ctx.datasource("name")` resolves to it.
//!   3. The proxy's `publish(msg)` method argument-checks + calls
//!      `Rivers.__brokerPublish("<name>", msg)`.
//!   4. The callback lazily creates the `BrokerProducer` on first publish in
//!      the task; cached in `TASK_DIRECT_BROKER_PRODUCERS`; closed in
//!      `TaskLocals::drop`.

use std::collections::HashMap;

use rivers_runtime::rivers_driver_sdk::broker::{
    BrokerConsumerConfig, OutboundMessage,
};

/// Minimal producer-side config. The BrokerConsumerConfig struct is also used
/// by `create_producer` in the trait signature but the producer path only
/// reads the reconnect_ms + node_id fields; empty subscriptions are fine.
fn producer_config() -> BrokerConsumerConfig {
    BrokerConsumerConfig {
        group_prefix: String::new(),
        app_id: String::new(),
        datasource_id: String::new(),
        node_id: String::new(),
        reconnect_ms: 0,
        subscriptions: Vec::new(),
    }
}

use super::http::json_to_v8;
use super::task_locals::*;

/// V8 callback for `Rivers.__brokerPublish(name, msg)`.
///
/// Arguments:
/// - `name` (string) — broker datasource name.
/// - `msg` (object) — fields `{destination, payload, headers?, key?, reply_to?}`.
///   `payload` may be a string (taken as UTF-8 bytes) or an object (JSON-stringified).
///
/// Returns a JS object `{id: string | null, metadata: string | null}` on success.
/// Throws `Error` on DriverError with the underlying message.
pub(super) fn rivers_broker_publish_callback(
    scope: &mut v8::HandleScope,
    args: v8::FunctionCallbackArguments,
    mut rv: v8::ReturnValue,
) {
    // ── Extract + validate args ──────────────────────────────────
    let name = args.get(0).to_rust_string_lossy(scope);
    if name.is_empty() {
        throw_type_error(scope, "__brokerPublish: 'name' is required");
        return;
    }

    let msg_val = args.get(1);
    if !msg_val.is_object() {
        throw_type_error(scope, "__brokerPublish: 'msg' must be an object");
        return;
    }

    // JSON-stringify the message object to drive into a serde_json::Value.
    let Some(msg_json) = v8::json::stringify(scope, msg_val) else {
        throw_type_error(scope, "__brokerPublish: could not serialise msg");
        return;
    };
    let msg_str = msg_json.to_rust_string_lossy(scope);
    let parsed: serde_json::Value = match serde_json::from_str(&msg_str) {
        Ok(v) => v,
        Err(e) => {
            throw_type_error(scope, &format!("__brokerPublish: malformed msg: {e}"));
            return;
        }
    };

    let message = match build_outbound_message(&parsed) {
        Ok(m) => m,
        Err(e) => {
            throw_type_error(scope, &format!("__brokerPublish: {e}"));
            return;
        }
    };

    // ── Resolve runtime + driver factory ─────────────────────────
    let rt = match get_rt_handle() {
        Ok(rt) => rt,
        Err(_) => {
            throw_type_error(scope, "__brokerPublish: tokio runtime handle unavailable");
            return;
        }
    };
    let Some(factory) = TASK_DRIVER_FACTORY.with(|f| f.borrow().clone()) else {
        throw_type_error(scope, "__brokerPublish: driver factory unavailable");
        return;
    };

    // ── Publish via cached or lazy-created producer ──────────────
    let result = TASK_DIRECT_BROKER_PRODUCERS.with(|m| -> Result<rivers_runtime::rivers_driver_sdk::broker::PublishReceipt, String> {
        let map = m.borrow();
        let entry = map.get(&name).ok_or_else(|| {
            format!("datasource '{name}' is not a broker datasource")
        })?;

        // Lazy-init the producer on first publish in this task.
        if entry.producer.borrow().is_none() {
            let driver = factory.get_broker_driver(&entry.driver).ok_or_else(|| {
                format!(
                    "broker driver '{}' not registered in DriverFactory",
                    entry.driver
                )
            })?;
            let cfg = producer_config();
            let producer = rt.block_on(async {
                driver
                    .create_producer(&entry.params, &cfg)
                    .await
                    .map_err(|e| format!("create_producer failed: {e}"))
            })?;
            *entry.producer.borrow_mut() = Some(producer);
        }

        let mut prod_slot = entry.producer.borrow_mut();
        let prod = prod_slot.as_mut().expect("producer initialised above");
        rt.block_on(prod.publish(message))
            .map_err(|e| format!("publish failed: {e}"))
    });

    match result {
        Ok(receipt) => {
            let mut obj = serde_json::Map::with_capacity(2);
            obj.insert(
                "id".into(),
                receipt
                    .id
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            );
            obj.insert(
                "metadata".into(),
                receipt
                    .metadata
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            );
            match json_to_v8(scope, &serde_json::Value::Object(obj)) {
                Ok(v) => rv.set(v),
                Err(_) => rv.set(v8::null(scope).into()),
            }
        }
        Err(msg) => {
            // DriverError surfaces as a plain Error (not TypeError) so
            // handlers can distinguish argument mistakes from runtime failures.
            throw_error(scope, &msg);
        }
    }
}

/// Build an `OutboundMessage` from a parsed JSON message object.
fn build_outbound_message(val: &serde_json::Value) -> Result<OutboundMessage, String> {
    let obj = val.as_object().ok_or("msg must be an object")?;

    let destination = obj
        .get("destination")
        .and_then(|v| v.as_str())
        .ok_or("'destination' is required (string)")?
        .to_string();
    if destination.is_empty() {
        return Err("'destination' must be non-empty".into());
    }

    let payload = match obj.get("payload") {
        Some(serde_json::Value::String(s)) => s.as_bytes().to_vec(),
        Some(serde_json::Value::Null) | None => {
            return Err("'payload' is required".into());
        }
        Some(other) => {
            // Auto-stringify objects/arrays/numbers/bools (BR0.3 decision).
            serde_json::to_vec(other).map_err(|e| format!("payload serialisation: {e}"))?
        }
    };

    let headers: HashMap<String, String> = obj
        .get("headers")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let key = obj
        .get("key")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let reply_to = obj
        .get("reply_to")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(OutboundMessage {
        destination,
        payload,
        headers,
        key,
        reply_to,
    })
}

// ── Proxy codegen ────────────────────────────────────────────────

/// Build the JS snippet for a broker datasource proxy.
///
/// Emits a `publish(msg)` method that argument-checks and calls
/// `Rivers.__brokerPublish(ds_name, msg)`.
pub(super) fn build_broker_proxy_script(ds_name: &str) -> String {
    let mut out = String::with_capacity(ds_name.len() + 320);
    out.push_str("(function(){const proxy={};proxy.publish=function(msg){");
    out.push_str("if(msg===undefined||msg===null||typeof msg!==\"object\"){");
    out.push_str("throw new TypeError(\"publish: 'msg' must be an object\");}");
    out.push_str("if(typeof msg.destination!==\"string\"||msg.destination===\"\"){");
    out.push_str("throw new TypeError(\"publish: 'destination' is required\");}");
    out.push_str("if(msg.payload===undefined||msg.payload===null){");
    out.push_str("throw new TypeError(\"publish: 'payload' is required\");}");
    out.push_str("return Rivers.__brokerPublish(");
    push_js_string(&mut out, ds_name);
    out.push_str(",msg);};return proxy;})()");
    out
}

fn push_js_string(out: &mut String, raw: &str) {
    out.push('"');
    for c in raw.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

// ── Helpers ──────────────────────────────────────────────────────

fn throw_type_error(scope: &mut v8::HandleScope, msg: &str) {
    if let Some(s) = v8::String::new(scope, msg) {
        let err = v8::Exception::type_error(scope, s);
        scope.throw_exception(err);
    }
}

fn throw_error(scope: &mut v8::HandleScope, msg: &str) {
    if let Some(s) = v8::String::new(scope, msg) {
        let err = v8::Exception::error(scope, s);
        scope.throw_exception(err);
    }
}

// ── Unit tests ───────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn br2_t_build_outbound_destination_required() {
        let j: serde_json::Value = serde_json::json!({"payload": "x"});
        let err = build_outbound_message(&j).unwrap_err();
        assert!(err.contains("destination"), "err={err}");
    }

    #[test]
    fn br2_t_build_outbound_payload_required() {
        let j: serde_json::Value = serde_json::json!({"destination": "topic"});
        let err = build_outbound_message(&j).unwrap_err();
        assert!(err.contains("payload"), "err={err}");
    }

    #[test]
    fn br2_t_build_outbound_string_payload_is_utf8_bytes() {
        let j: serde_json::Value = serde_json::json!({
            "destination": "topic",
            "payload": "hello"
        });
        let m = build_outbound_message(&j).unwrap();
        assert_eq!(m.payload, b"hello");
        assert_eq!(m.destination, "topic");
        assert!(m.headers.is_empty());
        assert!(m.key.is_none());
    }

    #[test]
    fn br2_t_build_outbound_object_payload_is_json_stringified() {
        let j: serde_json::Value = serde_json::json!({
            "destination": "topic",
            "payload": {"a": 1, "b": "two"}
        });
        let m = build_outbound_message(&j).unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&m.payload).unwrap();
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["b"], "two");
    }

    #[test]
    fn br2_t_build_outbound_headers_and_key_pass_through() {
        let j: serde_json::Value = serde_json::json!({
            "destination": "topic",
            "payload": "x",
            "headers": {"h1": "v1", "h2": "v2"},
            "key": "partition-key",
            "reply_to": "reply.topic"
        });
        let m = build_outbound_message(&j).unwrap();
        assert_eq!(m.headers.get("h1"), Some(&"v1".to_string()));
        assert_eq!(m.headers.get("h2"), Some(&"v2".to_string()));
        assert_eq!(m.key.as_deref(), Some("partition-key"));
        assert_eq!(m.reply_to.as_deref(), Some("reply.topic"));
    }

    #[test]
    fn br2_t_build_outbound_empty_destination_rejected() {
        let j: serde_json::Value = serde_json::json!({"destination": "", "payload": "x"});
        let err = build_outbound_message(&j).unwrap_err();
        assert!(err.contains("non-empty"), "err={err}");
    }

    #[test]
    fn br2_t_proxy_script_emits_publish_with_destination_check() {
        let s = build_broker_proxy_script("kafka");
        assert!(s.starts_with("(function(){"));
        assert!(s.ends_with(")()"));
        assert!(s.contains("proxy.publish=function(msg)"));
        assert!(s.contains("'destination' is required"));
        assert!(s.contains("'payload' is required"));
        assert!(s.contains("Rivers.__brokerPublish(\"kafka\""));
    }

    #[test]
    fn br2_t_proxy_script_escapes_datasource_name() {
        let s = build_broker_proxy_script(r#"weird"name"#);
        assert!(s.contains(r#"\""#), "escaping: {s}");
    }
}
