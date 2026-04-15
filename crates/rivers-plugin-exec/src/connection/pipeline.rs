//! 11-step execution pipeline for `ExecConnection`.
//!
//! This `impl ExecConnection` block adds `execute_command()` — the method
//! that `Connection::execute()` delegates to. Steps: command lookup, args
//! extraction, schema validation, integrity check, semaphore acquisition
//! (global then per-command), process execution, result mapping.

use std::collections::HashMap;
use std::time::Instant;

use rivers_driver_sdk::{DriverError, Query, QueryResult, QueryValue};

use crate::executor;

use super::exec_connection::ExecConnection;

// ── 11-Step Pipeline ─────────────────────────────────────────────────

impl ExecConnection {
    pub(crate) async fn execute_command(&self, query: &Query) -> Result<QueryResult, DriverError> {
        // Step 1: Extract command name from query
        let command_name = query
            .parameters
            .get("command")
            .and_then(|v| match v {
                QueryValue::String(s) => Some(s.as_str()),
                _ => None,
            })
            .or_else(|| {
                if !query.statement.is_empty() {
                    Some(query.statement.as_str())
                } else {
                    None
                }
            })
            .ok_or_else(|| DriverError::Query("missing 'command' parameter".into()))?;

        // Extract trace_id from query parameters (if provided), else use "-"
        let trace_id = query
            .parameters
            .get("trace_id")
            .and_then(|v| match v {
                QueryValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "-".into());

        let start = Instant::now();

        tracing::info!(
            datasource = "exec",
            command = %command_name,
            trace_id = %trace_id,
            "exec: command start"
        );

        // Step 2: Lookup command
        let cmd = self.commands.get(command_name).ok_or_else(|| {
            DriverError::Unsupported(format!("unknown command: '{command_name}'"))
        })?;

        // Step 3: Extract args from query parameters
        let args = query
            .parameters
            .get("args")
            .and_then(|v| match v {
                QueryValue::Json(j) => Some(j.clone()),
                _ => None,
            })
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        // Step 4: Schema validation (if declared)
        if let Some(ref compiled_schema) = cmd.schema {
            compiled_schema.validate(&args)?;
        }

        // Step 5: Integrity check (mode-dependent)
        if cmd.integrity.should_check() {
            if let Err(e) = cmd.integrity.verify(&cmd.config.path) {
                tracing::error!(
                    datasource = "exec",
                    command = %command_name,
                    trace_id = %trace_id,
                    "exec: integrity check failed"
                );
                return Err(e);
            }
        }

        // Step 6: Acquire global semaphore
        let _global_permit = self.global_semaphore.try_acquire().map_err(|_| {
            tracing::warn!(
                datasource = "exec",
                command = %command_name,
                trace_id = %trace_id,
                "exec: concurrency limit reached"
            );
            DriverError::Query(format!(
                "concurrency limit reached for command '{command_name}'"
            ))
        })?;

        // Step 7: Acquire per-command semaphore (if configured)
        // On failure, _global_permit is dropped automatically (RAII), releasing
        // the global permit before the error propagates to the caller.
        let _cmd_permit = if let Some(ref sem) = cmd.semaphore {
            Some(sem.try_acquire().map_err(|_| {
                tracing::warn!(
                    datasource = "exec",
                    command = %command_name,
                    trace_id = %trace_id,
                    "exec: concurrency limit reached"
                );
                DriverError::Query(format!(
                    "concurrency limit reached for command '{command_name}'"
                ))
            })?)
        } else {
            None
        };

        // Steps 8-11: Execute process (spawn, write stdin, read stdout, evaluate)
        match executor::execute_command(&cmd.config, &self.config, &args).await {
            Ok(result) => {
                let duration = start.elapsed().as_millis();
                tracing::info!(
                    datasource = "exec",
                    command = %command_name,
                    trace_id = %trace_id,
                    duration_ms = %duration,
                    exit_code = 0,
                    "exec: command success"
                );

                // Permits released automatically when _global_permit and _cmd_permit drop

                // Map JSON result to QueryResult.
                // QueryResult has no raw_value field, so wrap as a single row with
                // key "result" -> QueryValue::Json(parsed_json).
                let mut row = HashMap::new();
                row.insert("result".to_string(), QueryValue::Json(result));

                Ok(QueryResult {
                    rows: vec![row],
                    affected_rows: 1,
                    last_insert_id: None,
                    column_names: None,
                })
            }
            Err(e) => {
                let duration = start.elapsed().as_millis();
                let err_msg = e.to_string();

                // Categorize log level based on error type
                if err_msg.contains("command timed out") {
                    let timeout_ms = cmd
                        .config
                        .timeout_ms
                        .unwrap_or(self.config.default_timeout_ms);
                    tracing::warn!(
                        datasource = "exec",
                        command = %command_name,
                        trace_id = %trace_id,
                        timeout_ms = %timeout_ms,
                        "exec: command timed out"
                    );
                } else if err_msg.contains("output exceeded limit") {
                    tracing::warn!(
                        datasource = "exec",
                        command = %command_name,
                        trace_id = %trace_id,
                        "exec: output exceeded limit"
                    );
                } else {
                    tracing::error!(
                        datasource = "exec",
                        command = %command_name,
                        trace_id = %trace_id,
                        duration_ms = %duration,
                        error = %err_msg,
                        "exec: command failed"
                    );
                }

                Err(e)
            }
        }
    }
}
