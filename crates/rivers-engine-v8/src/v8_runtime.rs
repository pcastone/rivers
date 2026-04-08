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
        // Block dynamic code generation from strings in the sandbox.
        // Prevents code injection via built-in constructors.
        v8::V8::set_flags_from_string("--disallow-code-generation-from-strings");

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

/// Maximum heap usage (as fraction of limit) before an isolate is discarded
/// instead of returned to the pool.
const HEAP_RECYCLE_THRESHOLD: f64 = 0.5;

thread_local! {
    static ISOLATE_POOL: RefCell<Vec<v8::OwnedIsolate>> = RefCell::new(Vec::new());
}

/// Flag to prevent multiple termination spawns from the heap callback.
/// Reset after each handler execution in `execution.rs`.
pub(crate) static HEAP_OOM_TRIGGERED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Callback invoked when V8 isolate approaches heap limit.
///
/// Spawns a thread to terminate execution after a tiny delay (letting V8's
/// GC finish cleanly). Grants 64MB headroom so V8 can process the termination
/// instead of immediately hitting the fatal OOM handler.
extern "C" fn near_heap_limit_callback(
    data: *mut std::ffi::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    if !HEAP_OOM_TRIGGERED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        // First trigger — spawn a thread to terminate from a clean context.
        if !data.is_null() {
            let isolate = unsafe { &mut *(data as *mut v8::Isolate) };
            let handle = isolate.thread_safe_handle();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1));
                handle.terminate_execution();
            });
        }
    }
    // Grant generous headroom for V8 to process the deferred termination
    current_heap_limit + 64 * 1024 * 1024
}

pub(crate) fn acquire_isolate(heap_limit: usize) -> v8::OwnedIsolate {
    ensure_v8_initialized();
    ISOLATE_POOL.with(|pool| {
        pool.borrow_mut().pop().unwrap_or_else(|| {
            let params = v8::CreateParams::default().heap_limits(0, heap_limit);
            let mut isolate = v8::Isolate::new(params);

            // Register heap limit callback — terminates execution on OOM
            let isolate_ptr = &mut *isolate as *mut v8::Isolate as *mut std::ffi::c_void;
            isolate.add_near_heap_limit_callback(near_heap_limit_callback, isolate_ptr);

            isolate
        })
    })
}

pub(crate) fn release_isolate(mut isolate: v8::OwnedIsolate) {
    // Check heap usage — discard if above threshold to prevent memory buildup
    let mut stats = v8::HeapStatistics::default();
    isolate.get_heap_statistics(&mut stats);
    let usage = stats.used_heap_size() as f64 / stats.heap_size_limit() as f64;
    if usage > HEAP_RECYCLE_THRESHOLD {
        drop(isolate);
        return;
    }

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
