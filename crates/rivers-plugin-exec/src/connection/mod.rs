//! ExecDriver connection and pipeline — wires the 11-step execution pipeline.
//!
//! `ExecDriver` implements `DatabaseDriver` (factory for connections).
//! `ExecConnection` implements `Connection` (the pipeline itself).
//!
//! Concurrency control (spec S12): two-layer semaphore system.
//! - Global semaphore on `ExecConnection` limits total concurrent commands.
//! - Per-command semaphore on `CommandRuntime` limits per-command concurrency.
//! - Acquisition order: global first, then per-command.
//! - `try_acquire()` — no queuing, immediate error if full.

pub mod driver;
pub mod exec_connection;
pub mod pipeline;

pub use driver::{CommandRuntime, ExecDriver};
pub use exec_connection::ExecConnection;

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use rivers_driver_sdk::{Connection, ConnectionParams, DatabaseDriver, Query, QueryValue};
    use sha2::{Digest, Sha256};
    #[allow(unused_imports)]
    use std::collections::HashMap;
    use std::io::Write as _;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    use crate::integrity::{self, CommandIntegrity};
    use crate::schema::CompiledSchema;

    /// Create a test script, make it executable, and return (path, sha256_hex).
    fn create_test_script(dir: &Path, name: &str, content: &str) -> (std::path::PathBuf, String) {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let hash: [u8; 32] = Sha256::digest(content.as_bytes()).into();
        let hex = hex::encode(hash);
        (path, hex)
    }

    /// Build ConnectionParams with the given options.
    fn make_params(opts: Vec<(&str, &str)>) -> ConnectionParams {
        ConnectionParams {
            host: "localhost".into(),
            port: 0,
            database: "".into(),
            username: "".into(),
            password: "".into(),
            options: opts
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    // ── ExecDriver.connect() ────────────────────────────────────────

    #[tokio::test]
    async fn connect_with_valid_script_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &hash),
            ("commands.echo.input_mode", "stdin"),
        ]);

        let driver = ExecDriver;
        let conn = driver.connect(&params).await;
        assert!(conn.is_ok(), "connect should succeed: {:?}", conn.err());
    }

    #[tokio::test]
    async fn connect_with_bad_hash_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, _hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let wrong_hash = "ff".repeat(32);
        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &wrong_hash),
            ("commands.echo.input_mode", "stdin"),
        ]);

        let driver = ExecDriver;
        let result = driver.connect(&params).await;
        assert!(result.is_err(), "connect should fail with hash mismatch");
        let msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected error"),
        };
        assert!(
            msg.contains("integrity check failed"),
            "unexpected error: {msg}"
        );
    }

    // ── Full pipeline (T7.5) ────────────────────────────────────────

    #[tokio::test]
    async fn full_pipeline_stdin_echo() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &hash),
            ("commands.echo.input_mode", "stdin"),
        ]);

        let driver = ExecDriver;
        let mut conn = driver.connect(&params).await.unwrap();

        // Build a query
        let query = Query::with_operation("query", "", "")
            .param("command", QueryValue::String("echo".into()))
            .param(
                "args",
                QueryValue::Json(serde_json::json!({"hello": "world", "count": 42})),
            );

        let result = conn.execute(&query).await.unwrap();
        assert_eq!(result.rows.len(), 1, "should have one row");
        assert_eq!(result.affected_rows, 1);

        let row = &result.rows[0];
        let result_value = row.get("result").expect("row should have 'result' key");
        match result_value {
            QueryValue::Json(json) => {
                assert_eq!(json, &serde_json::json!({"hello": "world", "count": 42}));
            }
            other => panic!("expected QueryValue::Json, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn full_pipeline_command_from_statement() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &hash),
            ("commands.echo.input_mode", "stdin"),
        ]);

        let driver = ExecDriver;
        let mut conn = driver.connect(&params).await.unwrap();

        // Use statement field instead of parameter for command name
        let query = Query::with_operation("query", "", "echo").param(
            "args",
            QueryValue::Json(serde_json::json!({"key": "value"})),
        );

        let result = conn.execute(&query).await.unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0]["result"] {
            QueryValue::Json(json) => {
                assert_eq!(json, &serde_json::json!({"key": "value"}));
            }
            other => panic!("expected Json, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn pipeline_unknown_command_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &hash),
            ("commands.echo.input_mode", "stdin"),
        ]);

        let driver = ExecDriver;
        let mut conn = driver.connect(&params).await.unwrap();

        let query = Query::with_operation("query", "", "")
            .param("command", QueryValue::String("nonexistent".into()));

        let err = conn.execute(&query).await.unwrap_err();
        match err {
            rivers_driver_sdk::DriverError::Unsupported(msg) => {
                assert!(
                    msg.contains("unknown command: 'nonexistent'"),
                    "unexpected: {msg}"
                );
            }
            other => panic!("expected Unsupported, got: {other}"),
        }
    }

    #[tokio::test]
    async fn pipeline_unsupported_operation_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &hash),
            ("commands.echo.input_mode", "stdin"),
        ]);

        let driver = ExecDriver;
        let mut conn = driver.connect(&params).await.unwrap();

        let query = Query::with_operation("insert", "", "echo");
        let err = conn.execute(&query).await.unwrap_err();
        match err {
            rivers_driver_sdk::DriverError::Unsupported(msg) => {
                assert!(
                    msg.contains("does not support 'insert'"),
                    "unexpected: {msg}"
                );
            }
            other => panic!("expected Unsupported, got: {other}"),
        }
    }

    #[tokio::test]
    async fn pipeline_missing_command_parameter_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &hash),
            ("commands.echo.input_mode", "stdin"),
        ]);

        let driver = ExecDriver;
        let mut conn = driver.connect(&params).await.unwrap();

        // No command parameter and empty statement
        let query = Query::with_operation("query", "", "");
        let err = conn.execute(&query).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("missing 'command' parameter"),
            "unexpected: {msg}"
        );
    }

    #[tokio::test]
    async fn ping_always_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &hash),
            ("commands.echo.input_mode", "stdin"),
        ]);

        let driver = ExecDriver;
        let mut conn = driver.connect(&params).await.unwrap();
        assert!(conn.ping().await.is_ok());
    }

    // ── Concurrency control (T6.5) ─────────────────────────────────

    /// Helper: build an ExecConnection directly (bypassing Box<dyn Connection>)
    /// so tests can access internal fields like semaphores.
    async fn make_connection(params: &ConnectionParams) -> ExecConnection {
        let config = crate::config::ExecConfig::parse(params).unwrap();
        config.validate().unwrap();

        let mut commands = HashMap::new();
        for (name, cmd_config) in &config.commands {
            let mode = cmd_config
                .integrity_check
                .as_ref()
                .unwrap_or(&config.integrity_check);
            let pinned =
                integrity::verify_at_startup(&cmd_config.path, &cmd_config.sha256).unwrap();
            let integrity_checker = CommandIntegrity::new(mode.clone(), pinned);
            integrity::log_integrity_mode("exec", name, mode);

            let compiled_schema = if let Some(ref schema_path) = cmd_config.args_schema {
                Some(CompiledSchema::load(schema_path).unwrap())
            } else {
                None
            };

            let semaphore = cmd_config.max_concurrent.map(|n| Arc::new(Semaphore::new(n)));

            commands.insert(
                name.clone(),
                CommandRuntime {
                    config: cmd_config.clone(),
                    integrity: integrity_checker,
                    schema: compiled_schema,
                    semaphore,
                },
            );
        }

        let global_semaphore = Arc::new(Semaphore::new(config.max_concurrent));

        ExecConnection {
            config,
            commands,
            global_semaphore,
        }
    }

    #[tokio::test]
    async fn global_concurrency_limit_enforced() {
        let dir = tempfile::tempdir().unwrap();
        // Script that sleeps briefly to hold the semaphore
        let (script_path, hash) = create_test_script(
            dir.path(),
            "slow.sh",
            "#!/bin/sh\nsleep 1\necho '{\"ok\":true}'\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("max_concurrent", "1"), // only 1 global slot
            ("commands.slow.path", script_path.to_str().unwrap()),
            ("commands.slow.sha256", &hash),
            ("commands.slow.input_mode", "stdin"),
            ("commands.slow.integrity_check", "startup_only"),
        ]);

        let mut conn = make_connection(&params).await;

        // Manually acquire the global semaphore to simulate a running command
        let _permit = conn.global_semaphore.clone().try_acquire_owned().unwrap();

        // Now try to execute — should fail immediately
        let query = Query::with_operation("query", "", "")
            .param("command", QueryValue::String("slow".into()));

        let err = conn.execute(&query).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("concurrency limit reached"),
            "unexpected: {msg}"
        );
    }

    #[tokio::test]
    async fn per_command_concurrency_limit_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "slow.sh",
            "#!/bin/sh\nsleep 1\necho '{\"ok\":true}'\n",
        );

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("max_concurrent", "10"), // plenty of global slots
            ("commands.slow.path", script_path.to_str().unwrap()),
            ("commands.slow.sha256", &hash),
            ("commands.slow.input_mode", "stdin"),
            ("commands.slow.max_concurrent", "1"), // only 1 per-command slot
            ("commands.slow.integrity_check", "startup_only"),
        ]);

        let mut conn = make_connection(&params).await;

        // Manually acquire the per-command semaphore
        let cmd_sem = conn.commands.get("slow").unwrap().semaphore.clone().unwrap();
        let _permit = cmd_sem.try_acquire_owned().unwrap();

        // Now try to execute — should fail on per-command limit
        let query = Query::with_operation("query", "", "")
            .param("command", QueryValue::String("slow".into()));

        let err = conn.execute(&query).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("concurrency limit reached"),
            "unexpected: {msg}"
        );
    }

    #[tokio::test]
    async fn schema_validation_in_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let (script_path, hash) = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        // Write a schema file
        let schema_path = dir.path().join("schema.json");
        {
            let mut f = std::fs::File::create(&schema_path).unwrap();
            f.write_all(
                br#"{
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": { "type": "string" }
                }
            }"#,
            )
            .unwrap();
        }

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.echo.path", script_path.to_str().unwrap()),
            ("commands.echo.sha256", &hash),
            ("commands.echo.input_mode", "stdin"),
            ("commands.echo.args_schema", schema_path.to_str().unwrap()),
        ]);

        let driver = ExecDriver;
        let mut conn = driver.connect(&params).await.unwrap();

        // Valid args - should succeed
        let query = Query::with_operation("query", "", "")
            .param("command", QueryValue::String("echo".into()))
            .param(
                "args",
                QueryValue::Json(serde_json::json!({"name": "Alice"})),
            );
        let result = conn.execute(&query).await;
        assert!(result.is_ok(), "valid args should pass: {:?}", result.err());

        // Invalid args - missing required "name" field
        let query = Query::with_operation("query", "", "")
            .param("command", QueryValue::String("echo".into()))
            .param(
                "args",
                QueryValue::Json(serde_json::json!({"other": "field"})),
            );
        let err = conn.execute(&query).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("schema validation failed"),
            "unexpected: {msg}"
        );
    }

    #[tokio::test]
    async fn args_mode_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let script_content = r#"#!/bin/sh
printf '['
first=true
for arg in "$@"; do
    if [ "$first" = true ]; then
        first=false
    else
        printf ','
    fi
    printf '"%s"' "$arg"
done
printf ']'
"#;
        let (script_path, hash) = create_test_script(dir.path(), "args.sh", script_content);

        let params = make_params(vec![
            ("run_as_user", "nobody"),
            ("working_directory", dir.path().to_str().unwrap()),
            ("commands.scan.path", script_path.to_str().unwrap()),
            ("commands.scan.sha256", &hash),
            ("commands.scan.input_mode", "args"),
            ("commands.scan.args_template.0", "--host"),
            ("commands.scan.args_template.1", "{host}"),
            ("commands.scan.args_template.2", "--port"),
            ("commands.scan.args_template.3", "{port}"),
        ]);

        let driver = ExecDriver;
        let mut conn = driver.connect(&params).await.unwrap();

        let query = Query::with_operation("query", "", "")
            .param("command", QueryValue::String("scan".into()))
            .param(
                "args",
                QueryValue::Json(serde_json::json!({"host": "example.com", "port": "8080"})),
            );

        let result = conn.execute(&query).await.unwrap();
        assert_eq!(result.rows.len(), 1);
        match &result.rows[0]["result"] {
            QueryValue::Json(json) => {
                assert_eq!(
                    json,
                    &serde_json::json!(["--host", "example.com", "--port", "8080"])
                );
            }
            other => panic!("expected Json, got {:?}", other),
        }
    }
}
