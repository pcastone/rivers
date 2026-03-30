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
/// Used to clean up the IsolateHandle passed to the near-heap-limit callback.
pub(super) struct RawPtrGuard(pub(super) *mut std::ffi::c_void);

impl Drop for RawPtrGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                drop(Box::from_raw(self.0 as *mut v8::IsolateHandle));
            }
        }
    }
}

/// Near-heap-limit callback for V8 isolates (P4.1).
///
/// When V8's heap approaches the configured limit, this callback terminates
/// execution via the IsolateHandle passed as the `data` pointer.  This
/// prevents V8 from hitting its fatal OOM handler (which aborts the process)
/// and instead causes a catchable termination exception.
///
/// We grant a small amount of extra headroom (5 MiB) so V8 has enough
/// memory to process the termination rather than immediately triggering
/// the fatal OOM handler.
pub(super) extern "C" fn near_heap_limit_cb(
    data: *mut std::ffi::c_void,
    current_heap_limit: usize,
    _initial_heap_limit: usize,
) -> usize {
    if !data.is_null() {
        let handle = unsafe { &*(data as *const v8::IsolateHandle) };
        handle.terminate_execution();
    }
    // Grant a small amount of extra headroom for the termination to propagate
    current_heap_limit + 5 * 1024 * 1024
}
