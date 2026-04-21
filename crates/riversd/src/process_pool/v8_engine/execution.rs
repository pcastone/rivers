//! Main execution entry point: `execute_js_task()`, `call_entrypoint()`,
//! ES module support, promise resolution, module source resolution.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;

use super::super::types::*;
use super::super::{ActiveTask, TaskTerminator};
use super::super::v8_config::compile_typescript;
use super::task_locals::TaskLocals;
use super::init::{
    v8_str, ensure_v8_initialized, acquire_isolate, release_isolate,
    RawPtrGuard, near_heap_limit_cb, HeapCallbackData, DEFAULT_HEAP_LIMIT,
};
use super::context::{inject_ctx_object, inject_ctx_methods};
use super::rivers_global::inject_rivers_global;
use super::http::v8_to_json;

type ActiveTaskRegistry = Arc<StdMutex<HashMap<usize, ActiveTask>>>;

// ── ES Module Support (T4) ──────────────────────────────────────

/// Execute a JavaScript module (supports import/export) using V8.
///
/// Used when the source contains `export` or `import` statements.
/// Per `rivers-processpool-runtime-spec-v2.md` -- ES module execution path.
fn execute_as_module(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    source: &str,
    module_name: &str,
) -> Result<(), TaskError> {
    let source_str = v8::String::new(scope, source)
        .ok_or_else(|| TaskError::Internal("failed to create module source string".into()))?;
    let resource_name = v8::String::new(scope, module_name)
        .ok_or_else(|| TaskError::Internal("failed to create module name string".into()))?;

    let origin = v8::ScriptOrigin::new(
        scope,
        resource_name.into(),
        0,     // line offset
        0,     // column offset
        false, // is shared cross-origin
        -1,    // script id
        None,  // source map URL
        false, // is opaque
        false, // is WASM
        true,  // is module
        None,  // host defined options
    );

    let mut v8_source = v8::script_compiler::Source::new(source_str, Some(&origin));

    let tc = &mut v8::TryCatch::new(scope);
    let module = v8::script_compiler::compile_module(tc, &mut v8_source).ok_or_else(|| {
        let msg = tc
            .exception()
            .map(|e| e.to_rust_string_lossy(tc))
            .unwrap_or_else(|| "unknown".into());
        TaskError::HandlerError(format!("module compilation failed: {msg}"))
    })?;

    // Register the root module's identity_hash → path so the V8 resolve
    // callback (spec §3.6) can discover the referrer's path during nested
    // resolves. The callback is an extern "C" fn and cannot capture state
    // through a closure, so this registry is the bridge.
    let root_path = std::path::PathBuf::from(module_name)
        .canonicalize()
        .unwrap_or_else(|_| std::path::PathBuf::from(module_name));
    let root_id = module.get_identity_hash().get();
    super::task_locals::TASK_MODULE_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(root_id, root_path);
    });

    // Resolve callback — spec §3.1–3.6. Closed over only through thread-locals.
    let instantiate_result = module.instantiate_module(tc, resolve_module_callback);

    if instantiate_result != Some(true) {
        let msg = tc
            .exception()
            .map(|e| e.to_rust_string_lossy(tc))
            .unwrap_or_else(|| "module instantiation failed".into());
        return Err(TaskError::HandlerError(format!(
            "module instantiation: {msg}"
        )));
    }

    // Evaluate the module
    let _result = module.evaluate(tc).ok_or_else(|| {
        let msg = tc
            .exception()
            .map(|e| e.to_rust_string_lossy(tc))
            .unwrap_or_else(|| "module evaluation failed".into());
        TaskError::HandlerError(format!("module evaluation: {msg}"))
    })?;

    // Pump microtask queue for top-level await
    tc.perform_microtask_checkpoint();

    // Spec §4: capture the module namespace so call_entrypoint can look up
    // `export function handler` without requiring `globalThis.handler = ...`.
    // v8::Global is a persistent handle — safe to stash in a thread-local.
    let namespace = module.get_module_namespace();
    let ns_obj = v8::Local::<v8::Object>::try_from(namespace)
        .map_err(|_| TaskError::Internal("module namespace is not an object".into()))?;
    let global = v8::Global::new(tc, ns_obj);
    super::task_locals::TASK_MODULE_NAMESPACE.with(|n| {
        *n.borrow_mut() = Some(global);
    });

    Ok(())
}

/// V8 module resolve callback — spec §3.1–3.6.
///
/// Rules:
/// - Specifier MUST start with `./` or `../` (no bare specifiers, no absolute)
/// - Specifier MUST carry an explicit `.ts` or `.js` extension
/// - Resolved path MUST exist in the bundle module cache
///   (cache residency is the implicit chroot — the cache only contains files
///    under each app's `libraries/` tree, so boundary is enforced for free)
/// - Throws a V8 Error if any rule fails; V8 propagates it out of
///   `instantiate_module` as the caught exception
fn resolve_module_callback<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _import_attributes: v8::Local<'s, v8::FixedArray>,
    referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
    // SAFETY: V8 invokes this callback within a live isolate+context; we wrap
    // that in a HandleScope via CallbackScope so we can manipulate V8 values.
    let scope = &mut unsafe { v8::CallbackScope::new(context) };

    let spec = specifier.to_rust_string_lossy(scope);

    let throw_resolve_error = |scope: &mut v8::HandleScope<'s>, msg: String|
        -> Option<v8::Local<'s, v8::Module>>
    {
        let err_str = v8::String::new(scope, &msg)?;
        let err = v8::Exception::error(scope, err_str);
        scope.throw_exception(err);
        None
    };

    // Spec §3.2: bare specifiers rejected (no node_modules resolution).
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return throw_resolve_error(
            scope,
            format!(
                "module resolution failed: bare specifier \"{spec}\" not supported — use \"./\" or \"../\" relative import"
            ),
        );
    }

    // Spec §3.1: explicit extension required.
    if !(spec.ends_with(".ts") || spec.ends_with(".js")) {
        return throw_resolve_error(
            scope,
            format!(
                "module resolution failed: import specifier \"{spec}\" has no extension; hint: add \".ts\" or \".js\""
            ),
        );
    }

    // Find the referrer's path via its identity hash in the module registry.
    let referrer_id = referrer.get_identity_hash().get();
    let referrer_path = super::task_locals::TASK_MODULE_REGISTRY.with(|reg| {
        reg.borrow().get(&referrer_id).cloned()
    });
    let Some(referrer_path) = referrer_path else {
        return throw_resolve_error(
            scope,
            format!("module resolution failed: cannot identify referrer of \"{spec}\"; module registry missing entry"),
        );
    };

    // Resolve against referrer's parent directory and canonicalise.
    let parent = referrer_path.parent().unwrap_or_else(|| std::path::Path::new("/"));
    let joined = parent.join(&spec);
    let abs = match joined.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return throw_resolve_error(
                scope,
                format!(
                    "module resolution failed: cannot resolve \"{spec}\" from {} — {e}",
                    referrer_path.display()
                ),
            );
        }
    };

    // Spec §3.4: look up in the bundle module cache. Cache residency is the
    // boundary check: if it's in the cache, it was walked from {app}/libraries/
    // at bundle load. Anything outside the boundary is not in the cache.
    let cache = super::super::module_cache::get_module_cache()?;
    let Some(entry) = cache.get(&abs) else {
        return throw_resolve_error(
            scope,
            format!(
                "module resolution failed: \"{spec}\" resolved to {} which is not in the bundle module cache (may be outside {{app}}/libraries/ or not pre-compiled)",
                abs.display()
            ),
        );
    };

    // Compile a v8::Module from the cached JS.
    let source_str = v8::String::new(scope, &entry.compiled_js)?;
    let name_str = v8::String::new(scope, &abs.to_string_lossy())?;
    let origin = v8::ScriptOrigin::new(
        scope,
        name_str.into(),
        0,
        0,
        false,
        -1,
        None,
        false,
        false,
        true,
        None,
    );
    let mut v8_source = v8::script_compiler::Source::new(source_str, Some(&origin));
    let resolved_module = v8::script_compiler::compile_module(scope, &mut v8_source)?;

    // Register this module's identity_hash → path so nested resolves work.
    let id = resolved_module.get_identity_hash().get();
    super::task_locals::TASK_MODULE_REGISTRY.with(|reg| {
        reg.borrow_mut().insert(id, abs);
    });

    Some(resolved_module)
}

/// Detect if source uses ES module syntax.
///
/// Checks for `export` or `import` keywords at statement boundaries.
pub(crate) fn is_module_syntax(source: &str) -> bool {
    source.contains("export ") || source.contains("export\n")
        || source.contains("import ") || source.contains("import\n")
}

/// If the return value is a Promise, resolve it by pumping the microtask queue.
///
/// Async handler functions return a Promise. This resolves the promise
/// synchronously by repeatedly pumping the V8 microtask queue until
/// the promise settles or a maximum iteration count is reached.
fn resolve_promise_if_needed(
    scope: &mut v8::HandleScope<'_>,
    value: v8::Local<v8::Value>,
) -> Result<serde_json::Value, TaskError> {
    if value.is_promise() {
        let promise = v8::Local::<v8::Promise>::try_from(value)
            .map_err(|_| TaskError::Internal("promise cast failed".into()))?;

        // Pump microtasks until the promise settles, with a bound to prevent infinite spinning
        let max_ticks = 10_000;
        for _ in 0..max_ticks {
            scope.perform_microtask_checkpoint();
            match promise.state() {
                v8::PromiseState::Fulfilled => {
                    let result = promise.result(scope);
                    return v8_to_json(scope, result);
                }
                v8::PromiseState::Rejected => {
                    let rejection = promise.result(scope);
                    let msg = rejection.to_rust_string_lossy(scope);
                    return Err(TaskError::HandlerError(format!(
                        "async handler rejected: {msg}"
                    )));
                }
                v8::PromiseState::Pending => continue,
            }
        }
        return Err(TaskError::Timeout(0));
    }
    // Not a promise -- convert directly
    v8_to_json(scope, value)
}

/// Execute a JavaScript task using the V8 engine.
///
/// V8 is synchronous from the Rust side, so execution happens on a
/// blocking thread via `tokio::task::spawn_blocking`.
///
/// The pool watchdog thread enforces the CPU timeout by calling
/// `isolate.terminate_execution()` if the handler exceeds `timeout_ms`.
pub(crate) async fn execute_js_task(
    ctx: TaskContext,
    timeout_ms: u64,
    worker_id: usize,
    heap_bytes: usize,
    heap_threshold: f64,
    registry: Option<ActiveTaskRegistry>,
) -> Result<TaskResult, TaskError> {
    ensure_v8_initialized();

    // Capture the tokio runtime handle BEFORE spawn_blocking (async context).
    // Inside spawn_blocking this handle enables V8 callbacks to call async code
    // via rt_handle.block_on().
    let rt_handle = tokio::runtime::Handle::current();

    let result = tokio::task::spawn_blocking(move || {
        // Set all task-scoped thread-locals and arm automatic cleanup.
        // TaskLocals::set populates every thread-local; its Drop impl clears
        // every one -- impossible to add a setup without a matching teardown.
        let _locals = TaskLocals::set(&ctx, rt_handle);

        let start = Instant::now();

        // V2.8: Acquire a recycled isolate from the pool (or create a new one).
        // X5.2: Use pool-configured heap limit instead of default.
        let effective_heap = if heap_bytes > 0 { heap_bytes } else { DEFAULT_HEAP_LIMIT };
        let mut isolate = acquire_isolate(effective_heap);

        // Phase 6: install the PrepareStackTrace callback on the isolate so
        // `Error.stack` reports original `.ts` positions remapped through
        // `sourcemap_cache`. On first access of `.stack` after a throw, V8
        // invokes `prepare_stack_trace_cb` with the structured CallSite[]
        // and expects back a Local<Value> to use as the stack string.
        isolate.set_prepare_stack_trace_callback(prepare_stack_trace_cb);

        // Install near-heap-limit callback with HeapCallbackData.
        // The callback sets oom_triggered flag + calls terminate_execution()
        // with extra headroom so V8 can propagate the termination cleanly.
        let heap_cb_data = Box::new(HeapCallbackData {
            handle: isolate.thread_safe_handle(),
            oom_triggered: std::sync::atomic::AtomicBool::new(false),
        });
        let heap_cb_ptr = Box::into_raw(heap_cb_data) as *mut std::ffi::c_void;
        let _heap_cb_guard = RawPtrGuard(heap_cb_ptr);
        isolate.add_near_heap_limit_callback(near_heap_limit_cb, heap_cb_ptr);

        // Wave 10: Register in pool watchdog registry for V8 timeout
        let isolate_handle = isolate.thread_safe_handle();
        if let Some(ref reg) = registry {
            reg.lock().unwrap().insert(worker_id, ActiveTask {
                started_at: start,
                timeout_ms,
                terminator: TaskTerminator::V8(isolate_handle.clone()),
            });
        }

        // Resolve source before entering V8 scopes (no V8 refs needed)
        let source = resolve_module_source(&ctx)?;

        // All V8 scope work in a block so scopes drop before pool release (V2.8).
        let task_result: Result<TaskResult, TaskError> = {
            let mut handle_scope = v8::HandleScope::new(&mut isolate);
            let context = v8::Context::new(&mut handle_scope, Default::default());
            let mut scope = v8::ContextScope::new(&mut handle_scope, context);

            // Inject the `ctx` and `__args` globals
            inject_ctx_object(&mut scope, &ctx)?;

            // Inject ctx.dataview(), ctx.store, ctx.datasource() methods (P3)
            inject_ctx_methods(&mut scope)?;

            // Inject Rivers global utilities (Rivers.log, Rivers.crypto, console)
            inject_rivers_global(&mut scope)?;

            // Choose script or module execution path.
            // Module mode: entrypoint is looked up on the module namespace
            // (spec §4). Classic-script mode: entrypoint on globalThis.
            if is_module_syntax(&source) {
                execute_as_module(&mut scope, &source, &ctx.entrypoint.module)?;
            } else {
                // Existing script-based execution
                let v8_source = v8::String::new(&mut scope, &source)
                    .ok_or_else(|| TaskError::Internal("failed to create V8 string".into()))?;

                let tc_compile = &mut v8::TryCatch::new(&mut scope);
                let script = v8::Script::compile(tc_compile, v8_source, None)
                    .ok_or_else(|| {
                        if tc_compile.has_terminated() {
                            return TaskError::Timeout(timeout_ms);
                        }
                        let msg = tc_compile
                            .exception()
                            .map(|e| e.to_rust_string_lossy(tc_compile))
                            .unwrap_or_else(|| "unknown".into());
                        TaskError::HandlerError(format!("JS compilation failed: {msg}"))
                    })?;
                script.run(tc_compile).ok_or_else(|| {
                    if tc_compile.has_terminated() {
                        return TaskError::Timeout(timeout_ms);
                    }
                    let msg = tc_compile
                        .exception()
                        .map(|e| e.to_rust_string_lossy(tc_compile))
                        .unwrap_or_else(|| "unknown".into());
                    TaskError::HandlerError(format!("JS top-level execution failed: {msg}"))
                })?;
            }

            // Call the entrypoint function (returns JSON via TryCatch)
            let return_value = call_entrypoint(&mut scope, &ctx.entrypoint.function);

            // Handle timeout detected during entrypoint call
            let return_value = match return_value {
                Err(TaskError::Timeout(_)) => return Err(TaskError::Timeout(timeout_ms)),
                other => other?,
            };

            // P1.1: Read ctx.resdata back from V8 (handler may have modified it).
            // If handler set ctx.resdata, use that. Otherwise use the return value.
            // This supports both:
            //   - Standard handlers: set ctx.resdata, return void
            //   - Guard handlers: return claims directly
            let ctx_key = v8_str(&mut scope, "ctx")?;
            let global = scope.get_current_context().global(&mut scope);
            if let Some(ctx_obj) = global.get(&mut scope, ctx_key.into()) {
                if let Ok(ctx_obj) = v8::Local::<v8::Object>::try_from(ctx_obj) {
                    let resdata_key = v8_str(&mut scope, "resdata")?;
                    if let Some(resdata_val) = ctx_obj.get(&mut scope, resdata_key.into()) {
                        if !resdata_val.is_null() && !resdata_val.is_undefined() {
                            // ctx.resdata was set by handler -- use it as the result
                            let json_value = v8_to_json(&mut scope, resdata_val)?;
                            let duration_ms = start.elapsed().as_millis() as u64;
                            return Ok(TaskResult { value: json_value, duration_ms });
                        }
                    }
                }
            }

            // Fall back to handler return value
            let duration_ms = start.elapsed().as_millis() as u64;
            Ok(TaskResult {
                value: return_value,
                duration_ms,
            })

        }; // <-- V8 scopes dropped here

        // Wave 10: Deregister from pool watchdog BEFORE touching the isolate.
        // This prevents the watchdog from calling terminate_execution() on
        // an isolate that's about to be dropped or recycled.
        if let Some(ref reg) = registry {
            reg.lock().unwrap().remove(&worker_id);
        }

        // Check if near-heap-limit callback fired (OOM condition).
        // If so, treat the isolate as tainted — do not recycle.
        let oom_hit = {
            let cb_data = unsafe { &*(heap_cb_ptr as *const HeapCallbackData) };
            cb_data.oom_triggered.load(std::sync::atomic::Ordering::SeqCst)
        };

        // V2.8: Return isolate to pool for reuse.
        // On timeout/error/OOM: drop the isolate (don't recycle — may be in bad state).
        // On success (no OOM): remove heap callback, check threshold, then recycle or drop.
        if task_result.is_ok() && !oom_hit {
            isolate.remove_near_heap_limit_callback(near_heap_limit_cb, effective_heap);

            let should_recycle = if heap_threshold > 0.0 && effective_heap > 0 {
                let mut stats = v8::HeapStatistics::default();
                isolate.get_heap_statistics(&mut stats);
                let threshold_bytes = (effective_heap as f64 * heap_threshold) as usize;
                stats.used_heap_size() > threshold_bytes
            } else {
                false
            };

            if should_recycle {
                tracing::debug!(
                    target: "rivers.pool",
                    worker_id = worker_id,
                    "recycling V8 isolate -- heap usage exceeded threshold"
                );
                // Drop isolate instead of returning to pool -- a fresh one will be created next time
                drop(isolate);
            } else {
                release_isolate(isolate);
            }
        } else {
            // Timeout or error — drop isolate without recycling.
            // A terminated/errored isolate may be in an inconsistent state.
            drop(isolate);
        }

        task_result
    })
    .await
    .map_err(|e| TaskError::WorkerCrash(format!("worker {worker_id} panicked: {e}")))?;

    result
}

/// Call the named entrypoint function with `ctx` as the sole argument.
///
/// Uses a `v8::TryCatch` scope to capture exception details including
/// error messages. Returns `serde_json::Value` to avoid V8 lifetime
/// escape issues with the TryCatch scope.
fn call_entrypoint(
    scope: &mut v8::ContextScope<'_, v8::HandleScope<'_>>,
    function_name: &str,
) -> Result<serde_json::Value, TaskError> {
    // Spec §4: module-mode entrypoint lookup on the module namespace.
    // Classic-script mode falls through to the existing global-scope path.
    let namespace_local: Option<v8::Local<v8::Object>> =
        super::task_locals::TASK_MODULE_NAMESPACE.with(|n| {
            n.borrow().as_ref().map(|g| v8::Local::new(scope, g))
        });

    let func_key = v8::String::new(scope, function_name)
        .ok_or_else(|| TaskError::Internal(format!("failed to create key '{function_name}'")))?;

    let func_val = if let Some(ns) = namespace_local {
        ns.get(scope, func_key.into()).ok_or_else(|| {
            TaskError::HandlerError(format!(
                "exported function '{function_name}' not found on module namespace"
            ))
        })?
    } else {
        let global = scope.get_current_context().global(scope);
        global.get(scope, func_key.into()).ok_or_else(|| {
            TaskError::HandlerError(format!("function '{function_name}' not found"))
        })?
    };

    let func = v8::Local::<v8::Function>::try_from(func_val).map_err(|_| {
        TaskError::HandlerError(format!("'{function_name}' is not a function"))
    })?;

    // `ctx` is always injected on the global object (inject_ctx_object) —
    // even in module mode. Read from there regardless of entrypoint scope.
    let ctx_key = v8::String::new(scope, "ctx")
        .ok_or_else(|| TaskError::Internal("failed to create 'ctx' key".into()))?;
    let ctx_global = scope.get_current_context().global(scope);
    let ctx_val = ctx_global
        .get(scope, ctx_key.into())
        .ok_or_else(|| TaskError::Internal("ctx not found on global".into()))?;

    let undefined = v8::undefined(scope).into();

    // Use TryCatch to capture exception details
    let tc_scope = &mut v8::TryCatch::new(scope);
    match func.call(tc_scope, undefined, &[ctx_val]) {
        Some(result) => {
            // T4: If the return value is a Promise (async function), resolve it
            if result.is_promise() {
                return resolve_promise_if_needed(tc_scope, result);
            }
            // Convert to JSON inside the TryCatch scope
            if result.is_undefined() || result.is_null() {
                Ok(serde_json::Value::Null)
            } else {
                let json_str = v8::json::stringify(tc_scope, result)
                    .ok_or_else(|| TaskError::Internal("V8 JSON.stringify failed".into()))?;
                let rust_str = json_str.to_rust_string_lossy(tc_scope);
                serde_json::from_str(&rust_str)
                    .map_err(|e| TaskError::Internal(format!("parse JSON result: {e}")))
            }
        }
        None => {
            // Check for isolate termination (timeout)
            if tc_scope.has_terminated() {
                return Err(TaskError::Timeout(0));
            }
            let msg = if let Some(exception) = tc_scope.exception() {
                exception.to_rust_string_lossy(tc_scope)
            } else {
                "unknown exception".to_string()
            };
            Err(TaskError::HandlerError(format!(
                "handler '{function_name}' threw: {msg}"
            )))
        }
    }
}

/// Resolve the JavaScript source code for a task.
///
/// Per `docs/arch/rivers-javascript-typescript-spec.md` §2.6–2.8:
/// every `.ts`/`.js` under every app's `libraries/` is pre-compiled at
/// bundle load time into the process-global `BundleModuleCache`. This
/// function performs a cache lookup — not a live compilation.
///
/// Two fallback paths remain:
///
/// 1. `ctx.args["_source"]` — tests and dynamic-dispatch callers may inject
///    source inline without a disk file. TypeScript is compiled on the fly
///    via `compile_typescript()`; JS is used verbatim.
/// 2. Cache miss on `ctx.entrypoint.module` — read from disk and compile.
///    This is a defence-in-depth path for modules that exist on disk but
///    weren't walked (e.g., legacy handlers outside `libraries/`). Logged
///    so we can detect and fix such cases.
fn resolve_module_source(ctx: &TaskContext) -> Result<String, TaskError> {
    if let Some(source) = ctx.args.get("_source").and_then(|v| v.as_str()) {
        if ctx.entrypoint.language == "typescript" {
            return compile_typescript(source, &ctx.entrypoint.module);
        }
        return Ok(source.to_string());
    }

    let path = &ctx.entrypoint.module;

    // Primary path — consult the bundle module cache populated at load time.
    if let Some(cache) = super::super::module_cache::get_module_cache() {
        let abs = std::path::PathBuf::from(path)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(path));
        if let Some(entry) = cache.get(&abs) {
            return Ok(entry.compiled_js.clone());
        }
    }

    // Fallback — on-disk read + live compile. Should be rare after Phase 2.
    tracing::debug!(
        module = %path,
        "module cache miss — falling back to disk + live compile"
    );
    let source = std::fs::read_to_string(path)
        .map_err(|e| TaskError::HandlerError(format!("cannot read module '{path}': {e}")))?;

    let compiled = if path.ends_with(".ts") || ctx.entrypoint.language == "typescript" {
        compile_typescript(&source, path)?
    } else {
        source
    };

    Ok(compiled)
}

// ── Prepare Stack Trace Callback (spec §5.2) ────────────────────

/// Structured info extracted from a single V8 CallSite object.
///
/// Every field is `Option` because CallSite methods can return null for
/// native frames, eval frames, or when position metadata is absent.
#[derive(Debug, Default, Clone)]
struct CallSiteInfo {
    script_name: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
    function_name: Option<String>,
}

/// Call a no-arg method on a CallSite object via V8 reflection.
///
/// rusty_v8 doesn't wrap CallSite — the type-safe API is to look up the
/// method name on the object, cast to Function, and invoke with the
/// CallSite as `this`. Returns `None` if the lookup or call fails.
fn call_callsite_method<'s>(
    scope: &mut v8::HandleScope<'s>,
    site: v8::Local<'s, v8::Object>,
    method: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let name = v8::String::new(scope, method)?;
    let method_val = site.get(scope, name.into())?;
    let func = v8::Local::<v8::Function>::try_from(method_val).ok()?;
    func.call(scope, site.into(), &[])
}

/// Extract `(script_name, line, column, function_name)` from a CallSite.
///
/// V8 CallSite method reference:
/// https://v8.dev/docs/stack-trace-api — `getScriptName()`, `getLineNumber()`,
/// `getColumnNumber()`, `getFunctionName()`. Null/undefined returns become
/// `None` in the resulting `CallSiteInfo`.
fn extract_callsite<'s>(
    scope: &mut v8::HandleScope<'s>,
    site_val: v8::Local<'s, v8::Value>,
) -> CallSiteInfo {
    let mut info = CallSiteInfo::default();
    let Ok(site) = v8::Local::<v8::Object>::try_from(site_val) else {
        return info;
    };

    if let Some(v) = call_callsite_method(scope, site, "getScriptName") {
        if !v.is_null() && !v.is_undefined() {
            info.script_name = Some(v.to_rust_string_lossy(scope));
        }
    }
    if let Some(v) = call_callsite_method(scope, site, "getLineNumber") {
        if v.is_number() {
            info.line = v.uint32_value(scope);
        }
    }
    if let Some(v) = call_callsite_method(scope, site, "getColumnNumber") {
        if v.is_number() {
            info.column = v.uint32_value(scope);
        }
    }
    if let Some(v) = call_callsite_method(scope, site, "getFunctionName") {
        if !v.is_null() && !v.is_undefined() {
            info.function_name = Some(v.to_rust_string_lossy(scope));
        }
    }

    info
}

/// V8 `PrepareStackTraceCallback` — intercepts `Error.stack` construction
/// to rewrite frame positions from compiled-JS to original `.ts` coordinates.
///
/// Spec: `docs/arch/rivers-javascript-typescript-spec.md §5.2`. The
/// `sites` array is V8's structured CallSite list per
/// https://v8.dev/docs/stack-trace-api . V8 asserts the return is a
/// non-empty Local<Value>, so we always build a string even on failure.
///
/// For each frame, attempts source-map remap via `sourcemap_cache::get_or_parse`;
/// falls back to the unmapped compiled-JS position on cache miss, null
/// scriptName, or lookup failure.
fn prepare_stack_trace_cb<'s>(
    scope: &mut v8::HandleScope<'s>,
    error: v8::Local<'s, v8::Value>,
    sites: v8::Local<'s, v8::Array>,
) -> v8::Local<'s, v8::Value> {
    let mut out = error.to_rust_string_lossy(scope);
    let len = sites.length();
    for i in 0..len {
        let Some(site_val) = sites.get_index(scope, i) else {
            continue;
        };
        let info = extract_callsite(scope, site_val);
        out.push_str(&format_frame(&info));
    }
    v8::String::new(scope, &out)
        .map(|s| s.into())
        .unwrap_or_else(|| v8::String::empty(scope).into())
}

/// Format a single stack frame, remapping via the source-map cache when
/// possible. Falls back to the raw compiled-JS position otherwise.
///
/// Offset note: V8's CallSite positions are 1-based; swc_sourcemap's
/// `lookup_token` expects 0-based. The remapped output is re-incremented
/// back to 1-based for stack-trace convention.
fn format_frame(info: &CallSiteInfo) -> String {
    let fn_name = info
        .function_name
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or("<anonymous>");

    if let (Some(script), Some(line), Some(col)) =
        (info.script_name.as_deref(), info.line, info.column)
    {
        if line > 0 && col > 0 {
            let path = std::path::Path::new(script);
            if let Some(sm) = super::sourcemap_cache::get_or_parse(path) {
                if let Some(token) = sm.lookup_token(line - 1, col - 1) {
                    let src_line = token.get_src_line();
                    let src_col = token.get_src_col();
                    // Sentinel: u32::MAX = unmapped.
                    if src_line != u32::MAX && src_col != u32::MAX {
                        let src_file = token
                            .get_source()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| script.to_string());
                        return format!(
                            "\n    at {fn_name} ({src_file}:{}:{})",
                            src_line + 1,
                            src_col + 1
                        );
                    }
                }
            }
        }
    }

    // Unmapped / native / eval / cache-miss fallback.
    let script = info.script_name.as_deref().unwrap_or("<unknown>");
    let line = info.line.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
    let col = info.column.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
    format!("\n    at {fn_name} ({script}:{line}:{col})")
}

#[cfg(test)]
mod frame_format_tests {
    use super::{format_frame, CallSiteInfo};

    #[test]
    fn fallback_when_no_cache_entry() {
        // No source map cached for this path → unmapped format.
        let info = CallSiteInfo {
            script_name: Some("/never-installed/handler.ts".into()),
            line: Some(42),
            column: Some(5),
            function_name: Some("handler".into()),
        };
        let s = format_frame(&info);
        assert!(s.contains("handler"), "fn name: {s}");
        assert!(s.contains(":42:5"), "unmapped 1-based position: {s}");
        assert!(s.contains("/never-installed/handler.ts"), "raw path: {s}");
    }

    #[test]
    fn anonymous_when_no_function_name() {
        let info = CallSiteInfo {
            script_name: None,
            line: None,
            column: None,
            function_name: None,
        };
        let s = format_frame(&info);
        assert!(s.contains("<anonymous>"), "anon placeholder: {s}");
        assert!(s.contains("<unknown>"), "unknown script placeholder: {s}");
        assert!(s.contains(":?:?"), "unknown position placeholders: {s}");
    }

    #[test]
    fn zero_line_or_col_falls_back() {
        // line/col of 0 are invalid positions (V8 uses 1-based) — fall
        // through to unmapped to avoid u32 underflow at `line - 1`.
        let info = CallSiteInfo {
            script_name: Some("/some-file.ts".into()),
            line: Some(0),
            column: Some(0),
            function_name: Some("f".into()),
        };
        let s = format_frame(&info);
        assert!(s.contains(":0:0"), "unmapped retained 0s: {s}");
    }
}
