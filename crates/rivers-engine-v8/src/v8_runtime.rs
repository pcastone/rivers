//! V8 initialization, isolate pool, script cache, and helpers.
//!
//! Manages the V8 platform lifecycle, per-thread isolate pooling,
//! compiled script caching, and low-level V8 utility functions.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;

// ── V8 Initialization ───────────────────────────────────────────

static V8_INIT: std::sync::Once = std::sync::Once::new();

pub(crate) fn ensure_v8_initialized() {
    V8_INIT.call_once(|| {
        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

// ── Script Source Cache ─────────────────────────────────────────

pub(crate) static SCRIPT_CACHE: std::sync::LazyLock<StdMutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| StdMutex::new(HashMap::new()));

// ── Isolate Pool ────────────────────────────────────────────────

pub(crate) const DEFAULT_HEAP_LIMIT: usize = 128 * 1024 * 1024;

thread_local! {
    static ISOLATE_POOL: RefCell<Vec<v8::OwnedIsolate>> = RefCell::new(Vec::new());
}

pub(crate) fn acquire_isolate(heap_limit: usize) -> v8::OwnedIsolate {
    ensure_v8_initialized();
    ISOLATE_POOL.with(|pool| {
        pool.borrow_mut().pop().unwrap_or_else(|| {
            let params = v8::CreateParams::default().heap_limits(0, heap_limit);
            v8::Isolate::new(params)
        })
    })
}

pub(crate) fn release_isolate(isolate: v8::OwnedIsolate) {
    ISOLATE_POOL.with(|pool| pool.borrow_mut().push(isolate));
}

// ── V8 Helpers ──────────────────────────────────────────────────

pub(crate) fn v8_str<'s>(scope: &mut v8::HandleScope<'s>, s: &str) -> v8::Local<'s, v8::String> {
    v8::String::new(scope, s).unwrap()
}

pub(crate) fn v8_to_json_value(scope: &mut v8::HandleScope, val: v8::Local<v8::Value>) -> serde_json::Value {
    let json_str = v8::json::stringify(scope, val);
    match json_str {
        Some(s) => {
            let rust_str = s.to_rust_string_lossy(scope);
            serde_json::from_str(&rust_str).unwrap_or(serde_json::Value::Null)
        }
        None => serde_json::Value::Null,
    }
}
