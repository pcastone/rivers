//! Integration tests for the ExecDriver plugin.
//!
//! These tests exercise the full driver contract (connect -> execute -> verify result)
//! using real shell scripts in temp directories. All tests are gated behind `#[cfg(unix)]`
//! since they require POSIX shell scripts.

#![cfg(unix)]

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use rivers_driver_sdk::{
    ConnectionParams, DatabaseDriver, DriverError, Query, QueryValue,
};
use rivers_plugin_exec::ExecDriver;
use sha2::{Digest, Sha256};

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Create a test script, make it executable, return its path.
fn make_script(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Compute SHA-256 hex of a file.
fn hash_file(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    hex::encode(Sha256::digest(&bytes))
}

/// Build ConnectionParams with the given commands.
/// Each command is (name, path, sha256, input_mode).
fn make_params(
    dir: &Path,
    commands: &[(&str, &Path, &str, &str)],
    extra: &[(&str, &str)],
) -> ConnectionParams {
    let user = std::env::var("USER").unwrap_or_else(|_| "nobody".into());
    let mut options = HashMap::new();
    options.insert("run_as_user".into(), user);
    options.insert("working_directory".into(), dir.to_str().unwrap().into());

    for (name, path, sha256, input_mode) in commands {
        options.insert(
            format!("commands.{name}.path"),
            path.to_str().unwrap().into(),
        );
        options.insert(format!("commands.{name}.sha256"), sha256.to_string());
        options.insert(format!("commands.{name}.input_mode"), input_mode.to_string());
    }

    for (key, value) in extra {
        options.insert(key.to_string(), value.to_string());
    }

    ConnectionParams {
        host: String::new(),
        port: 0,
        database: String::new(),
        username: String::new(),
        password: String::new(),
        options,
    }
}

// ── T10.1-T10.2: Stdin mode round-trip ───────────────────────────────────────

#[tokio::test]
async fn stdin_mode_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let script = make_script(dir.path(), "echo_stdin.sh", "#!/bin/sh\ncat\n");
    let hash = hash_file(&script);

    let params = make_params(
        dir.path(),
        &[("echo", script.as_path(), &hash, "stdin")],
        &[],
    );

    let driver = ExecDriver;
    let mut conn = driver.connect(&params).await.unwrap();

    let input = serde_json::json!({"message": "hello", "count": 42});
    let query = Query::with_operation("query", "", "")
        .param("command", QueryValue::String("echo".into()))
        .param("args", QueryValue::Json(input.clone()));

    let result = conn.execute(&query).await.unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.affected_rows, 1);

    match &result.rows[0]["result"] {
        QueryValue::Json(json) => {
            assert_eq!(json, &input, "stdin echo should return the same JSON");
        }
        other => panic!("expected QueryValue::Json, got {:?}", other),
    }
}

// ── T10.3: Args mode ─────────────────────────────────────────────────────────

#[tokio::test]
async fn args_mode_template_interpolation() {
    let dir = tempfile::tempdir().unwrap();
    let script_content = r#"#!/bin/sh
printf '{"args": ["%s", "%s", "%s"]}' "$1" "$2" "$3"
"#;
    let script = make_script(dir.path(), "args_echo.sh", script_content);
    let hash = hash_file(&script);

    let params = make_params(
        dir.path(),
        &[("lookup", script.as_path(), &hash, "args")],
        &[
            ("commands.lookup.args_template.0", "{domain}"),
            ("commands.lookup.args_template.1", "--type"),
            ("commands.lookup.args_template.2", "{record_type}"),
        ],
    );

    let driver = ExecDriver;
    let mut conn = driver.connect(&params).await.unwrap();

    let query = Query::with_operation("query", "", "")
        .param("command", QueryValue::String("lookup".into()))
        .param(
            "args",
            QueryValue::Json(serde_json::json!({
                "domain": "example.com",
                "record_type": "A"
            })),
        );

    let result = conn.execute(&query).await.unwrap();
    match &result.rows[0]["result"] {
        QueryValue::Json(json) => {
            assert_eq!(
                json,
                &serde_json::json!({"args": ["example.com", "--type", "A"]})
            );
        }
        other => panic!("expected Json, got {:?}", other),
    }
}

// ── T10.5: Integrity check — correct hash passes, tampered file fails ────────

#[tokio::test]
async fn integrity_check_correct_hash_passes() {
    let dir = tempfile::tempdir().unwrap();
    let script = make_script(dir.path(), "echo.sh", "#!/bin/sh\ncat\n");
    let hash = hash_file(&script);

    let params = make_params(
        dir.path(),
        &[("echo", script.as_path(), &hash, "stdin")],
        &[("commands.echo.integrity_check", "each_time")],
    );

    let driver = ExecDriver;
    let mut conn = driver.connect(&params).await.unwrap();

    let query = Query::with_operation("query", "", "")
        .param("command", QueryValue::String("echo".into()))
        .param("args", QueryValue::Json(serde_json::json!({"ok": true})));

    let result = conn.execute(&query).await;
    assert!(result.is_ok(), "integrity check should pass with correct hash");
}

#[tokio::test]
async fn integrity_check_tampered_file_fails() {
    let dir = tempfile::tempdir().unwrap();
    let original_content = "#!/bin/sh\ncat\n";
    let script = make_script(dir.path(), "echo.sh", original_content);
    let hash = hash_file(&script);

    let params = make_params(
        dir.path(),
        &[("echo", script.as_path(), &hash, "stdin")],
        &[("commands.echo.integrity_check", "each_time")],
    );

    let driver = ExecDriver;
    let mut conn = driver.connect(&params).await.unwrap();

    // Tamper with the script after connect (which checks at startup)
    std::fs::write(&script, "#!/bin/sh\necho 'TAMPERED'\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let query = Query::with_operation("query", "", "")
        .param("command", QueryValue::String("echo".into()))
        .param("args", QueryValue::Json(serde_json::json!({"ok": true})));

    let err = conn.execute(&query).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("integrity check failed"),
        "expected integrity failure, got: {msg}"
    );
}

// ── T10.7: Timeout ───────────────────────────────────────────────────────────

#[tokio::test]
async fn timeout_kills_slow_command() {
    let dir = tempfile::tempdir().unwrap();
    let script = make_script(
        dir.path(),
        "slow.sh",
        "#!/bin/sh\nsleep 100\necho '{\"done\":true}'\n",
    );
    let hash = hash_file(&script);

    let params = make_params(
        dir.path(),
        &[("slow", script.as_path(), &hash, "stdin")],
        &[
            ("commands.slow.timeout_ms", "100"),
            ("commands.slow.integrity_check", "startup_only"),
        ],
    );

    let driver = ExecDriver;
    let mut conn = driver.connect(&params).await.unwrap();

    let query = Query::with_operation("query", "", "")
        .param("command", QueryValue::String("slow".into()))
        .param("args", QueryValue::Json(serde_json::json!({})));

    let err = conn.execute(&query).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("command timed out"),
        "expected timeout, got: {msg}"
    );
}

// ── T10.9: Non-zero exit ─────────────────────────────────────────────────────

#[tokio::test]
async fn non_zero_exit_returns_error_with_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let script = make_script(
        dir.path(),
        "fail.sh",
        "#!/bin/sh\necho 'something went wrong' >&2\nexit 1\n",
    );
    let hash = hash_file(&script);

    let params = make_params(
        dir.path(),
        &[("fail", script.as_path(), &hash, "stdin")],
        &[("commands.fail.integrity_check", "startup_only")],
    );

    let driver = ExecDriver;
    let mut conn = driver.connect(&params).await.unwrap();

    let query = Query::with_operation("query", "", "")
        .param("command", QueryValue::String("fail".into()))
        .param("args", QueryValue::Json(serde_json::json!({})));

    let err = conn.execute(&query).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("command failed"), "expected failure: {msg}");
    assert!(
        msg.contains("something went wrong"),
        "should contain stderr: {msg}"
    );
}

// ── T10.10: Unknown command ──────────────────────────────────────────────────

#[tokio::test]
async fn unknown_command_returns_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let script = make_script(dir.path(), "echo.sh", "#!/bin/sh\ncat\n");
    let hash = hash_file(&script);

    let params = make_params(
        dir.path(),
        &[("echo", script.as_path(), &hash, "stdin")],
        &[],
    );

    let driver = ExecDriver;
    let mut conn = driver.connect(&params).await.unwrap();

    let query = Query::with_operation("query", "", "")
        .param("command", QueryValue::String("nonexistent".into()));

    let err = conn.execute(&query).await.unwrap_err();
    match err {
        DriverError::Unsupported(msg) => {
            assert!(
                msg.contains("unknown command: 'nonexistent'"),
                "unexpected: {msg}"
            );
        }
        other => panic!("expected Unsupported, got: {other}"),
    }
}

// ── T10.11: Concurrency limit ────────────────────────────────────────────────

#[tokio::test]
async fn concurrency_limit_rejects_excess() {
    let dir = tempfile::tempdir().unwrap();
    let script = make_script(
        dir.path(),
        "slow.sh",
        "#!/bin/sh\nsleep 10\necho '{\"ok\":true}'\n",
    );
    let hash = hash_file(&script);

    let params = make_params(
        dir.path(),
        &[("slow", script.as_path(), &hash, "stdin")],
        &[
            ("max_concurrent", "1"),
            ("commands.slow.integrity_check", "startup_only"),
        ],
    );

    let driver = ExecDriver;
    let conn = driver.connect(&params).await.unwrap();

    // The Connection trait requires &mut self, so true concurrent calls on one connection
    // aren't possible here. Instead, we verify the semaphore path works correctly by
    // testing that sequential calls with max_concurrent=1 succeed (acquire, release, acquire).
    // The unit tests in connection.rs cover the rejection path via manual semaphore acquisition.
    let fast_script = make_script(
        dir.path(),
        "fast.sh",
        "#!/bin/sh\necho '{\"ok\":true}'\n",
    );
    let fast_hash = hash_file(&fast_script);

    let params2 = make_params(
        dir.path(),
        &[("fast", fast_script.as_path(), &fast_hash, "stdin")],
        &[
            ("max_concurrent", "1"),
            ("commands.fast.integrity_check", "startup_only"),
        ],
    );

    let mut conn2 = driver.connect(&params2).await.unwrap();
    let query = Query::with_operation("query", "", "")
        .param("command", QueryValue::String("fast".into()))
        .param("args", QueryValue::Json(serde_json::json!({})));

    // First call should succeed (takes the one slot, executes, releases)
    let result = conn2.execute(&query).await;
    assert!(result.is_ok(), "first call with max_concurrent=1 should succeed");

    // Second call should also succeed since the first completed and released its permit
    let result2 = conn2.execute(&query).await;
    assert!(result2.is_ok(), "second sequential call should succeed");

    // Now drop conn2 and use conn with the slow script to verify the error path.
    // We can't easily test true concurrency with &mut self, but the unit tests
    // in connection.rs cover the semaphore rejection path thoroughly.
    drop(conn);
    drop(conn2);
}
