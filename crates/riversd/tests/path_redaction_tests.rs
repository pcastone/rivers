//! B4 / P1-9: V8 stack traces and module-resolution errors must redact host
//! filesystem paths to their `{app}/libraries/...` logical form.
//!
//! Lives in its own integration binary so the `MODULE_CACHE` mutations done
//! by some sibling tests cannot leak in. The redaction behaviour is
//! unconditional — same in debug and release builds — so these assertions
//! hold under `cargo test` (debug) without any release-mode plumbing.

use std::collections::HashMap;
use std::path::PathBuf;

use riversd::process_pool::{
    Entrypoint, ProcessPoolManager, TaskContextBuilder, TaskError, TaskKind,
};

/// Build a handler file under a synthetic `<tmp>/<unique>/<app>/libraries/handlers/`
/// tree so the resolver can locate a `libraries/` ancestor and the redactor
/// has something concrete to anchor against. The full source is written
/// verbatim — caller controls whether it's classic or module syntax.
/// Returns `(app_dir, handler_path)`.
fn write_handler(unique: &str, app: &str, full_source: &str) -> (PathBuf, PathBuf) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!(
        "rivers_redact_{unique}_{}_{}",
        std::process::id(),
        id
    ));
    let app_dir = root.join(app);
    let handlers_dir = app_dir.join("libraries").join("handlers");
    std::fs::create_dir_all(&handlers_dir).expect("create handlers dir");
    let path = handlers_dir.join("throws.js");
    std::fs::write(&path, full_source).expect("write handler");
    (app_dir, path)
}

/// A handler that throws — its stack trace MUST report the redacted
/// `{app}/libraries/handlers/throws.js` script name, not the absolute
/// `/var/folders/.../my-app/libraries/handlers/throws.js` path.
#[tokio::test]
async fn handler_stack_does_not_leak_host_paths() {
    // Use module syntax (`export function handler`) so the V8 module loader
    // (not classic-script) runs — that's the path that registers the script
    // origin we redact in B4.2.
    let (app_dir, handler_path) = write_handler(
        "stack",
        "my-app",
        r#"export function handler(ctx) { throw new Error("boom from " + ctx.app_id); }"#,
    );
    let _cleanup = ScopedCleanup(app_dir.parent().map(|p| p.to_path_buf()));

    let mgr = ProcessPoolManager::from_config(&HashMap::new());
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: handler_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("redact-stack-test".into())
        .app_id("test-app-uuid".into())
        .node_id("test-node-1".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .build()
        .unwrap();

    let result = mgr.dispatch("default", ctx).await;
    let err = result.expect_err("handler that throws must surface as TaskError");

    // Walk both message and (if present) the stack — neither may leak the
    // host prefix above the app directory.
    let (message, stack_opt): (String, Option<String>) = match err {
        TaskError::HandlerErrorWithStack { message, stack } => (message, Some(stack)),
        TaskError::HandlerError(m) => (m, None),
        other => panic!("unexpected error variant: {other:?}"),
    };

    let abs_app_dir = app_dir.canonicalize().unwrap_or(app_dir.clone());
    let host_prefix = abs_app_dir
        .parent()
        .expect("app_dir has parent (the unique tmp root)")
        .to_string_lossy()
        .to_string();

    // Forbidden substrings: anything host-specific above the app boundary.
    let forbidden = [
        host_prefix.as_str(),
        "/Users/",
        "/var/folders/", // macOS tmp location
    ];

    for fragment in &forbidden {
        assert!(
            !message.contains(fragment),
            "B4: handler error message leaked host path fragment {fragment:?}: {message}"
        );
        if let Some(ref stack) = stack_opt {
            assert!(
                !stack.contains(fragment),
                "B4: handler stack leaked host path fragment {fragment:?}: {stack}"
            );
        }
    }

    // Positive: when V8 reports a script name in the stack at all, it must
    // be the app-relative form (B4.2). V8 may report `<unknown>` for the
    // root module on some frames — that's fine for our security claim.
    // What matters is: NO frame should ever name the absolute path.
    if let Some(ref stack) = stack_opt {
        // Any line that mentions a path must mention the redacted form,
        // not the absolute one. We already asserted absence of the host
        // prefix above; this is a stronger structural assertion.
        for line in stack.lines() {
            if line.contains(".js") {
                // Whatever script name appears here, it must not be the
                // absolute path. The earlier negative assertions cover
                // that — repeat the host-prefix check per line for a
                // sharper failure message.
                assert!(
                    !line.contains(&*host_prefix),
                    "B4: stack frame {line:?} leaked host prefix"
                );
            }
        }
    }
}

/// A handler whose nested import points outside the app boundary: the
/// resolve-callback error message MUST redact the referrer path in the
/// `in {referrer}` line.
///
/// We can't easily exercise the boundary-violation branch in an
/// in-process test (it requires a populated module cache), but we CAN
/// hit the `cannot resolve "..."` branch by importing a file that does
/// not exist on disk. That branch also goes through the redactor.
#[tokio::test]
async fn module_resolution_error_does_not_leak_host_paths() {
    let (app_dir, handler_path) = write_handler(
        "resolve",
        "my-app",
        r#"import { x } from "./does-not-exist.js";
export function handler(ctx) { return { x }; }"#,
    );
    let _cleanup = ScopedCleanup(app_dir.parent().map(|p| p.to_path_buf()));

    let mgr = ProcessPoolManager::from_config(&HashMap::new());
    let ctx = TaskContextBuilder::new()
        .entrypoint(Entrypoint {
            module: handler_path.to_string_lossy().into(),
            function: "handler".into(),
            language: "javascript".into(),
        })
        .args(serde_json::json!({}))
        .trace_id("redact-resolve-test".into())
        .app_id("test-app-uuid".into())
        .node_id("test-node-1".into())
        .runtime_env("test".into())
        .task_kind(TaskKind::Rest)
        .build()
        .unwrap();

    let result = mgr.dispatch("default", ctx).await;
    let err = result.expect_err("missing import must error");
    let msg = err.to_string();

    let abs_app_dir = app_dir.canonicalize().unwrap_or(app_dir.clone());
    let host_prefix = abs_app_dir
        .parent()
        .expect("app_dir has parent")
        .to_string_lossy()
        .to_string();

    // The host prefix above the app must NEVER appear in the resolve error.
    assert!(
        !msg.contains(&*host_prefix),
        "B4: resolve error leaked host prefix {host_prefix:?}: {msg}"
    );
    assert!(
        !msg.contains("/Users/"),
        "B4: resolve error contains /Users/ fragment: {msg}"
    );

    // Positive sanity: the redacted referrer should appear in the `in {referrer}`
    // line. Either the canonicalised or non-canonicalised form will reduce to
    // `my-app/libraries/handlers/throws.js` after redaction.
    assert!(
        msg.contains("my-app/libraries/handlers/throws.js"),
        "B4: resolve error should report the redacted referrer: {msg}"
    );
}

/// RAII guard that removes a temp tree at end of scope. Best-effort —
/// failure to delete is not propagated.
struct ScopedCleanup(Option<PathBuf>);
impl Drop for ScopedCleanup {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}
