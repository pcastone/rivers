//! V8 initialization, isolate pool, script cache, heap limit callback.

use std::cell::RefCell;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::sync::Mutex as StdMutex;

use super::super::types::*;

/// Create a V8 string, returning TaskError if it fails.
///
/// `v8::String::new()` returns `None` only if the string exceeds V8's
/// internal limit (~512 MB).  All call-sites pass short constant or
/// runtime-bounded strings, so failure is effectively impossible -- but
/// propagating an error is more idiomatic than `.unwrap()`.
pub(super) fn v8_str<'s>(
    scope: &mut v8::HandleScope<'s>,
    s: &str,
) -> Result<v8::Local<'s, v8::String>, TaskError> {
    v8::String::new(scope, s)
        .ok_or_else(|| TaskError::Internal(format!("V8 string creation failed for '{}'", s)))
}

/// One-time V8 platform initialization.
static V8_INIT: std::sync::Once = std::sync::Once::new();

pub(crate) fn ensure_v8_initialized() {
    V8_INIT.call_once(|| {
        // Block dynamic code generation from strings in the sandbox.
        // Prevents code injection via Function() constructor and similar APIs.
        //
        // Note: V8 13.0.245.12 (crate v130.0.7) has `js_decorators` defined
        // as EMPTY_INITIALIZE_GLOBAL_FOR_FEATURE — the flag is a placeholder
        // and the parser.cc has no `@`-token handling. TC39 Stage 3 decorator
        // syntax is not supported in this V8 build. The canary decorator test
        // (RT-TS-DECORATOR) uses the manual application pattern instead.
        v8::V8::set_flags_from_string("--disallow-code-generation-from-strings");

        let platform = v8::new_default_platform(0, false).make_shared();
        v8::V8::initialize_platform(platform);
        v8::V8::initialize();
    });
}

// ── V2.8: Isolate Pool ─────────────────────────────────────────

/// Default heap limit per isolate: 128 MiB.
pub(crate) const DEFAULT_HEAP_LIMIT: usize = 128 * 1024 * 1024;

thread_local! {
    /// Thread-local pool of reusable V8 isolates (V2.8).
    static ISOLATE_POOL: RefCell<Vec<v8::OwnedIsolate>> = RefCell::new(Vec::new());
}

/// Acquire an isolate from the thread-local pool, or create a fresh one.
pub(super) fn acquire_isolate(heap_limit: usize) -> v8::OwnedIsolate {
    ensure_v8_initialized();
    ISOLATE_POOL.with(|pool| {
        pool.borrow_mut().pop().unwrap_or_else(|| {
            let params = v8::CreateParams::default().heap_limits(0, heap_limit);
            v8::Isolate::new(params)
        })
    })
}

/// Return an isolate to the thread-local pool for reuse.
pub(super) fn release_isolate(isolate: v8::OwnedIsolate) {
    ISOLATE_POOL.with(|pool| {
        pool.borrow_mut().push(isolate);
    });
}

// ── V2.9: Script Source Cache ───────────────────────────────────
//
// SCRIPT_CACHE and clear_script_cache() are test-only -- no production code path uses them.

#[cfg(test)]
pub(crate) static SCRIPT_CACHE: std::sync::LazyLock<StdMutex<HashMap<String, String>>> =
    std::sync::LazyLock::new(|| StdMutex::new(HashMap::new()));

#[cfg(test)]
pub(crate) fn clear_script_cache() {
    if let Ok(mut cache) = SCRIPT_CACHE.lock() {
        cache.clear();
    }
}

/// RAII guard that frees a raw pointer when dropped.
/// Used to clean up the HeapCallbackData passed to the near-heap-limit callback.
pub(super) struct RawPtrGuard(pub(super) *mut std::ffi::c_void);

impl Drop for RawPtrGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                drop(Box::from_raw(self.0 as *mut HeapCallbackData));
            }
        }
    }
}

/// Data passed to the near-heap-limit callback.
///
/// Contains both the IsolateHandle for termination and an atomic flag
/// so the callback can signal OOM without calling terminate_execution()
/// directly from V8's GC thread (which can abort the process).
pub(super) struct HeapCallbackData {
    /// Handle to terminate the isolate (called from the WATCHDOG thread, not GC).
    pub handle: v8::IsolateHandle,
    /// Set to true by the heap callback; checked by the watchdog and post-call code.
    pub oom_triggered: std::sync::atomic::AtomicBool,
}

/// Near-heap-limit callback for V8 isolates (P4.1).
///
/// Instead of calling `terminate_execution()` directly (which can crash
/// when called from V8's GC thread), this callback:
/// 1. Sets `oom_triggered` flag for the watchdog/post-call check
/// 2. Calls `terminate_execution()` via the IsolateHandle (safe from here
///    because we also grant headroom)
/// 3. Grants extra headroom so V8 can process the termination
///
/// The combination of flag + terminate + headroom ensures:
/// - The watchdog sees OOM immediately and won't recycle the isolate
/// - V8 has enough memory to propagate the termination cleanly
/// - If terminate_execution() is unsafe in this context, the headroom
///   gives V8 room to return to user code where the flag is checked
pub(super) extern "C" fn near_heap_limit_cb(
    data: *mut std::ffi::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    if !data.is_null() {
        let cb_data = unsafe { &*(data as *const HeapCallbackData) };
        if !cb_data.oom_triggered.swap(true, std::sync::atomic::Ordering::SeqCst) {
            // First trigger — log it, then spawn a thread that terminates
            // from a clean context (not V8's GC thread).
            eprintln!("[HEAP-GUARD] near_heap_limit_cb fired at {}MB, granting 64MB headroom",
                current_heap_limit / (1024 * 1024));
            let handle = cb_data.handle.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1));
                handle.terminate_execution();
            });
        }
    }
    // Grant generous headroom so V8 can:
    // 1. Finish the current GC cycle
    // 2. Return to user code
    // 3. Process the termination from the spawned thread
    current_heap_limit + 64 * 1024 * 1024
}

