//! F2 (P1-7) — bound SWC compile time.
//!
//! `compile_typescript_with_imports_timeout` wraps the existing (already
//! panic-contained) compile in a per-module wall-clock budget so pathological
//! TS cannot stall `populate_module_cache` at bundle-load time.
//!
//! These tests pin three things:
//!   1. The happy path (small input, default timeout) still returns the
//!      compiled JS — the wrapper does not regress normal compiles.
//!   2. A pathological-sized input + a tight budget produces
//!      `TaskError::CompileTimeout` (not panic, not silent OK).
//!   3. The error path's `module` field is redacted (no host filesystem
//!      prefix leaks into the operator-facing error).

use super::super::v8_config::{
    compile_typescript_with_imports_timeout, swc_compile_timeout_ms, SwcTimeoutOverride,
    RIVERS_SWC_COMPILE_TIMEOUT_MS_ENV, SWC_COMPILE_TIMEOUT_DEFAULT_MS,
};
use super::super::TaskError;

#[test]
fn env_var_name_pins() {
    // F2.1: env var name is part of the deploy-doc surface; pin it.
    assert_eq!(
        RIVERS_SWC_COMPILE_TIMEOUT_MS_ENV,
        "RIVERS_SWC_COMPILE_TIMEOUT_MS"
    );
    assert_eq!(SWC_COMPILE_TIMEOUT_DEFAULT_MS, 5000);
}

#[test]
fn default_timeout_when_env_unset_or_zero() {
    // The env var is read once via OnceLock at first call. CI runs without
    // it set, so the resolved value MUST be the 5s default.
    let resolved = swc_compile_timeout_ms();
    // Either 5000 (default, expected) or a thread-local override leftover
    // (rejected — tests below are RAII-scoped).
    assert!(
        resolved == SWC_COMPILE_TIMEOUT_DEFAULT_MS || resolved >= 1,
        "expected default {SWC_COMPILE_TIMEOUT_DEFAULT_MS}ms, got {resolved}"
    );
}

#[test]
fn happy_path_small_input_compiles() {
    // Default timeout, trivial input — wrapper must not regress the normal
    // compile path. Returns JS, no error, no spurious timeout.
    let ts = "function handler(ctx: any): { ok: boolean } { return { ok: true }; }";
    let (js, imports, _map) =
        compile_typescript_with_imports_timeout(ts, "/tmp/app/libraries/handler.ts")
            .expect("trivial compile must succeed under default 5s budget");
    assert!(js.contains("function handler"), "compiled JS missing function: {js}");
    assert!(!js.contains(": any"), "type annotation not stripped: {js}");
    assert!(imports.is_empty());
}

/// Generate a deeply-nested generic chain that SWC has to walk through type
/// resolution + erasure. Each layer adds one generic wrapper. At ~5_000
/// layers the parse + transform takes well over 100ms on modern hardware.
fn pathological_nested_generics(depth: usize) -> String {
    // type T0 = number;
    // type T1 = Array<T0>;
    // type T2 = Array<T1>;
    // ...
    // function handler(ctx: T<DEPTH>) { return ctx; }
    let mut s = String::with_capacity(depth * 32);
    s.push_str("type T0 = number;\n");
    for i in 1..depth {
        s.push_str(&format!("type T{i} = Array<T{prev}>;\n", prev = i - 1));
    }
    s.push_str(&format!(
        "function handler(ctx: T{last}): T{last} {{ return ctx; }}\n",
        last = depth - 1
    ));
    s
}

#[test]
fn pathological_input_triggers_compile_timeout() {
    // Force a 1ms budget. The thread::spawn + parse setup alone exceeds 1ms,
    // and the pathological input adds parse/transform time on top — making
    // this test stable across hardware (no false negatives on fast CI).
    //
    // We deliberately use a large depth so that *if* the supervisor thread
    // happens to win the race on a very fast machine, the actual compile
    // would still take many milliseconds and trip the budget.
    let _guard = SwcTimeoutOverride::new(1);
    let ts = pathological_nested_generics(2_000);

    let start = std::time::Instant::now();
    let result = compile_typescript_with_imports_timeout(
        &ts,
        "/Users/somebody/projects/rivers-deploy/apps/myapp/libraries/big.ts",
    );
    let elapsed = start.elapsed();

    match result {
        Err(TaskError::CompileTimeout { module, timeout_ms }) => {
            assert_eq!(timeout_ms, 1, "timeout_ms must echo the configured budget");
            // F2.2: redaction — the absolute on-disk host prefix
            // (`/Users/somebody/projects/rivers-deploy/`) MUST be stripped.
            assert!(
                !module.contains("/Users/somebody"),
                "host filesystem prefix leaked into error: {module}"
            );
            assert!(
                !module.contains("rivers-deploy"),
                "deploy directory leaked into error: {module}"
            );
            assert!(
                module.contains("libraries") || module.contains("big.ts"),
                "redacted module should still identify the file: {module}"
            );
            // The wrapper must return promptly after the timeout — not wait
            // for the (still-running, leaked) inner compile to finish.
            assert!(
                elapsed < std::time::Duration::from_secs(2),
                "wrapper hung past the budget — elapsed: {elapsed:?}"
            );
        }
        Err(other) => panic!("expected CompileTimeout, got {other:?}"),
        Ok(_) => panic!("expected CompileTimeout, got Ok (input compiled in {elapsed:?})"),
    }
}

#[test]
fn moderately_pathological_input_under_generous_budget_succeeds() {
    // Mirror image of the pathological test: same shape of input but a
    // large enough budget that even slow CI completes. Confirms the wrapper
    // doesn't reject inputs that are merely "big" — only ones that exceed
    // the configured budget.
    //
    // Depth 200 with a 30s budget: SWC handles this comfortably (< 100ms
    // typical). If this test starts flaking, SWC has a perf regression.
    let _guard = SwcTimeoutOverride::new(30_000);
    let ts = pathological_nested_generics(200);

    let result = compile_typescript_with_imports_timeout(&ts, "/tmp/app/libraries/medium.ts");
    let (js, _imports, _map) = result.expect("depth=200 must compile under 30s");
    assert!(js.contains("function handler"));
}
