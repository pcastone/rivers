//! Process spawning for ExecDriver commands (spec sections 10-11).
//!
//! All process spawning uses `tokio::process::Command` with an explicit argument
//! array. No shell is involved at any point. Processes run in their own session
//! (via `setsid`) so the entire process group can be killed on timeout or output
//! overflow.

use std::time::Duration;

use rivers_driver_sdk::DriverError;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::config::{CommandConfig, ExecConfig, InputMode};
use crate::template;

// ── Process group kill ────────────────────────────────────────────────

/// Kill an entire process group via SIGKILL (Unix only).
///
/// The child is spawned as a session leader (`setsid`), so its PID doubles
/// as the PGID. Sending a negative PID to `kill(2)` targets the group.
#[cfg(unix)]
fn kill_process_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: Option<u32>) {}

// ── Command execution ─────────────────────────────────────────────────

/// Execute a command per the ExecDriver pipeline (spec sections 10-11).
///
/// This function handles steps 6-10 of the pipeline:
/// - Build the process command with isolation settings
/// - Write stdin if applicable
/// - Read stdout/stderr with size limits and timeout
/// - Evaluate the exit status and parse the JSON result
pub async fn execute_command(
    config: &CommandConfig,
    global_config: &ExecConfig,
    params: &serde_json::Value,
) -> Result<serde_json::Value, DriverError> {
    let params_obj = params.as_object().ok_or_else(|| {
        DriverError::Query("exec driver: params must be a JSON object".into())
    })?;

    // ── Build command ─────────────────────────────────────────────────

    let mut cmd = Command::new(&config.path);

    // Args from template interpolation (Args and Both modes)
    if config.input_mode == InputMode::Args || config.input_mode == InputMode::Both {
        if let Some(ref tmpl) = config.args_template {
            // For Both mode, remove stdin_key from params before template interpolation
            // so the stdin value doesn't leak into CLI arguments (Gap 10)
            let interpolation_params = if config.input_mode == InputMode::Both {
                let mut filtered = params_obj.clone();
                if let Some(ref key) = config.stdin_key {
                    filtered.remove(key);
                }
                filtered
            } else {
                params_obj.clone()
            };
            let args = template::interpolate(tmpl, &interpolation_params)?;
            cmd.args(&args);
        }
    }

    // Working directory
    cmd.current_dir(&global_config.working_directory);

    // Environment (spec section 11.2)
    if config.env_clear {
        cmd.env_clear();
        // Copy only allowed vars from host environment
        for var_name in &config.env_allow {
            if let Ok(val) = std::env::var(var_name) {
                cmd.env(var_name, val);
            }
        }
    }
    // Apply explicit env_set (overrides even allowed vars)
    for (key, value) in &config.env_set {
        cmd.env(key, value);
    }

    // Stdin configuration
    match config.input_mode {
        InputMode::Stdin | InputMode::Both => {
            cmd.stdin(std::process::Stdio::piped());
        }
        InputMode::Args => {
            cmd.stdin(std::process::Stdio::null());
        }
    }

    // Stdout and stderr always piped
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Kill child if the future is dropped
    cmd.kill_on_drop(true);

    // UID/GID privilege drop — only attempt if running as root (spec section 11.1)
    // For a plugin context, privilege drop requires root. If not root, skip with a
    // warning. Full implementation deferred to connection-level setup.
    #[cfg(unix)]
    {
        // Create a new session so the child becomes a process group leader.
        // This allows kill_process_group to send SIGKILL to all descendants.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }

        // Privilege drop: only if current process is root
        if nix_is_root() {
            if let Some((uid, gid)) = resolve_user(&global_config.run_as_user) {
                cmd.uid(uid);
                cmd.gid(gid);
            } else {
                tracing::warn!(
                    run_as_user = %global_config.run_as_user,
                    "could not resolve run_as_user — skipping privilege drop"
                );
            }
        } else {
            tracing::debug!(
                run_as_user = %global_config.run_as_user,
                "not running as root — skipping privilege drop"
            );
        }
    }

    // ── Spawn ─────────────────────────────────────────────────────────

    let mut child = cmd.spawn().map_err(|e| {
        DriverError::Internal(format!("failed to spawn command '{}': {e}", config.path.display()))
    })?;

    let child_pid = child.id();

    // ── Write stdin (spec section 10 step 8) ──────────────────────────

    match config.input_mode {
        InputMode::Stdin => {
            // Serialize entire params as JSON on stdin
            let json_bytes = serde_json::to_vec(params).map_err(|e| {
                DriverError::Query(format!("failed to serialize params to JSON: {e}"))
            })?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(&json_bytes).await.map_err(|e| {
                    DriverError::Query(format!("failed to write to stdin: {e}"))
                })?;
                // Drop stdin to signal EOF
            }
        }
        InputMode::Both => {
            // Extract stdin_key value, send on stdin as JSON
            let stdin_key = config.stdin_key.as_deref().unwrap_or("stdin");
            let stdin_value = params_obj.get(stdin_key).cloned().unwrap_or(serde_json::Value::Null);
            let json_bytes = serde_json::to_vec(&stdin_value).map_err(|e| {
                DriverError::Query(format!("failed to serialize stdin value to JSON: {e}"))
            })?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(&json_bytes).await.map_err(|e| {
                    DriverError::Query(format!("failed to write to stdin: {e}"))
                })?;
                // Drop stdin to signal EOF
            }
        }
        InputMode::Args => {
            // No stdin (already Stdio::null)
        }
    }

    // ── Bounded read with timeout (spec section 10 step 9) ────────────

    let timeout_ms = config.timeout_ms.unwrap_or(global_config.default_timeout_ms);
    let max_stdout = config.max_stdout_bytes.unwrap_or(global_config.max_stdout_bytes);

    match tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        // Read stdout with size limit
        let mut stdout_buf = Vec::with_capacity(max_stdout.min(65536));
        let mut stdout_reader = child.stdout.take().ok_or_else(|| {
            DriverError::Query("failed to capture stdout".into())
        })?;

        loop {
            let mut chunk = [0u8; 8192];
            let n = stdout_reader.read(&mut chunk).await.map_err(|e| {
                DriverError::Query(format!("failed to read stdout: {e}"))
            })?;
            if n == 0 {
                break;
            }
            stdout_buf.extend_from_slice(&chunk[..n]);
            if stdout_buf.len() > max_stdout {
                kill_process_group(child_pid);
                return Err(DriverError::Query("output exceeded limit".into()));
            }
        }

        // Read stderr (bounded to 64KB)
        let mut stderr_buf = vec![0u8; 65536];
        let mut stderr_reader = child.stderr.take().ok_or_else(|| {
            DriverError::Query("failed to capture stderr".into())
        })?;
        let stderr_n = stderr_reader.read(&mut stderr_buf).await.map_err(|e| {
            DriverError::Query(format!("failed to read stderr: {e}"))
        })?;

        let exit = child.wait().await.map_err(|e| {
            DriverError::Query(format!("failed to wait for process: {e}"))
        })?;

        Ok((stdout_buf, stderr_buf[..stderr_n].to_vec(), exit))
    })
    .await
    {
        Ok(Ok((stdout, stderr, exit))) => {
            // ── Evaluate result (spec section 10 step 10) ─────────────
            evaluate_result(&stdout, &stderr, exit)
        }
        Ok(Err(e)) => Err(e),
        Err(_timeout) => {
            kill_process_group(child_pid);
            Err(DriverError::Query("command timed out".into()))
        }
    }
}

// ── Result evaluation ─────────────────────────────────────────────────

/// Evaluate process output: check exit status, validate JSON output.
fn evaluate_result(
    stdout: &[u8],
    stderr: &[u8],
    exit: std::process::ExitStatus,
) -> Result<serde_json::Value, DriverError> {
    if !exit.success() {
        let stderr_str = String::from_utf8_lossy(stderr);
        let truncated = &stderr_str[..stderr_str.len().min(1024)];
        return Err(DriverError::Query(format!(
            "command failed: exit {}: {}",
            exit.code().unwrap_or(-1),
            truncated
        )));
    }

    if stdout.is_empty() {
        return Err(DriverError::Query(
            "command produced no output".into(),
        ));
    }

    let parsed: serde_json::Value = serde_json::from_slice(stdout)
        .map_err(|_| DriverError::Query("command produced invalid JSON".into()))?;

    Ok(parsed)
}

// ── Unix helpers ──────────────────────────────────────────────────────

/// Check if the current process is running as root.
#[cfg(unix)]
fn nix_is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// Resolve a username to (uid, gid) using libc getpwnam.
#[cfg(unix)]
fn resolve_user(username: &str) -> Option<(u32, u32)> {
    use std::ffi::CString;
    let c_name = CString::new(username).ok()?;
    let pw = unsafe { libc::getpwnam(c_name.as_ptr()) };
    if pw.is_null() {
        return None;
    }
    let uid = unsafe { (*pw).pw_uid };
    let gid = unsafe { (*pw).pw_gid };
    Some((uid, gid))
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::config::{CommandConfig, ExecConfig, InputMode, IntegrityMode};
    use serde_json::json;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// Create a test script in the given directory, make it executable.
    fn create_test_script(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// Build a minimal ExecConfig pointing to the given temp directory.
    fn make_global_config(working_dir: &Path) -> ExecConfig {
        ExecConfig {
            run_as_user: "nobody".into(),
            working_directory: working_dir.to_path_buf(),
            default_timeout_ms: 5000,
            max_stdout_bytes: 1_048_576,
            max_concurrent: 10,
            integrity_check: IntegrityMode::EachTime,
            commands: HashMap::new(),
        }
    }

    /// Build a CommandConfig for a given script path and input mode.
    fn make_command_config(
        path: PathBuf,
        input_mode: InputMode,
        args_template: Option<Vec<String>>,
        stdin_key: Option<String>,
    ) -> CommandConfig {
        CommandConfig {
            path,
            sha256: "0".repeat(64),
            input_mode,
            args_template,
            stdin_key,
            args_schema: None,
            timeout_ms: None,
            max_stdout_bytes: None,
            max_concurrent: None,
            integrity_check: None,
            env_clear: true,
            env_allow: Vec::new(),
            env_set: HashMap::new(),
        }
    }

    // ── stdin mode ───────────────────────────────────────────────────

    #[tokio::test]
    async fn stdin_mode_echo_back() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(
            dir.path(),
            "echo_stdin.sh",
            "#!/bin/sh\ncat\n",
        );

        let global = make_global_config(dir.path());
        let cmd = make_command_config(script, InputMode::Stdin, None, None);
        let params = json!({"hello": "world", "count": 42});

        let result = execute_command(&cmd, &global, &params).await.unwrap();
        assert_eq!(result, params);
    }

    // ── args mode ────────────────────────────────────────────────────

    #[tokio::test]
    async fn args_mode_echoes_argv() {
        let dir = tempfile::tempdir().unwrap();
        // Script that outputs its arguments as a JSON array
        let script = create_test_script(
            dir.path(),
            "args_script.sh",
            r#"#!/bin/sh
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
"#,
        );

        let global = make_global_config(dir.path());
        let cmd = make_command_config(
            script,
            InputMode::Args,
            Some(vec!["--host".into(), "{host}".into(), "--port".into(), "{port}".into()]),
            None,
        );
        let params = json!({"host": "example.com", "port": "8080"});

        let result = execute_command(&cmd, &global, &params).await.unwrap();
        assert_eq!(result, json!(["--host", "example.com", "--port", "8080"]));
    }

    // ── non-zero exit ────────────────────────────────────────────────

    #[tokio::test]
    async fn non_zero_exit_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(
            dir.path(),
            "fail.sh",
            "#!/bin/sh\necho 'something went wrong' >&2\nexit 1\n",
        );

        let global = make_global_config(dir.path());
        let cmd = make_command_config(script, InputMode::Stdin, None, None);
        let params = json!({});

        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("command failed"), "unexpected error: {msg}");
        assert!(msg.contains("exit 1"), "should contain exit code: {msg}");
        assert!(
            msg.contains("something went wrong"),
            "should contain stderr: {msg}"
        );
    }

    // ── empty output ─────────────────────────────────────────────────

    #[tokio::test]
    async fn empty_output_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(
            dir.path(),
            "empty.sh",
            "#!/bin/sh\nexit 0\n",
        );

        let global = make_global_config(dir.path());
        let cmd = make_command_config(script, InputMode::Stdin, None, None);
        let params = json!({});

        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("command produced no output"),
            "unexpected error: {msg}"
        );
    }

    // ── invalid JSON output ──────────────────────────────────────────

    #[tokio::test]
    async fn invalid_json_output_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(
            dir.path(),
            "bad_json.sh",
            "#!/bin/sh\necho 'not json'\n",
        );

        let global = make_global_config(dir.path());
        let cmd = make_command_config(script, InputMode::Stdin, None, None);
        let params = json!({});

        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("command produced invalid JSON"),
            "unexpected error: {msg}"
        );
    }

    // ── timeout ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn timeout_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(
            dir.path(),
            "slow.sh",
            "#!/bin/sh\nsleep 10\necho '{\"done\":true}'\n",
        );

        let global = make_global_config(dir.path());
        let mut cmd = make_command_config(script, InputMode::Stdin, None, None);
        cmd.timeout_ms = Some(100); // 100ms timeout
        let params = json!({});

        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("command timed out"),
            "unexpected error: {msg}"
        );
    }

    // ── env_clear ────────────────────────────────────────────────────

    #[tokio::test]
    async fn env_clear_filters_environment() {
        let dir = tempfile::tempdir().unwrap();
        // Script that dumps env as JSON object
        let script = create_test_script(
            dir.path(),
            "env_dump.sh",
            r#"#!/bin/sh
printf '{'
first=true
env | while IFS='=' read -r key value; do
    if [ "$first" = true ]; then
        first=false
    else
        printf ','
    fi
    printf '"%s":"%s"' "$key" "$value"
done
printf '}'
"#,
        );

        // Set an env var that should NOT appear when env_clear is true
        std::env::set_var("RIVERS_TEST_SECRET", "do_not_leak");
        // Set one that SHOULD appear via env_allow
        std::env::set_var("RIVERS_TEST_ALLOWED", "visible");

        let global = make_global_config(dir.path());
        let mut cmd = make_command_config(script, InputMode::Args, Some(vec![]), None);
        cmd.env_clear = true;
        cmd.env_allow = vec!["RIVERS_TEST_ALLOWED".into()];
        cmd.env_set
            .insert("RIVERS_EXPLICIT".into(), "set_value".into());
        let params = json!({});

        let result = execute_command(&cmd, &global, &params).await.unwrap();

        // The secret should NOT be present
        assert!(
            result.get("RIVERS_TEST_SECRET").is_none(),
            "env_clear should have filtered RIVERS_TEST_SECRET"
        );

        // The allowed var should be present
        assert_eq!(
            result.get("RIVERS_TEST_ALLOWED").and_then(|v| v.as_str()),
            Some("visible"),
            "env_allow should pass RIVERS_TEST_ALLOWED through"
        );

        // The explicit env_set should be present
        assert_eq!(
            result.get("RIVERS_EXPLICIT").and_then(|v| v.as_str()),
            Some("set_value"),
            "env_set should inject RIVERS_EXPLICIT"
        );

        // Clean up
        std::env::remove_var("RIVERS_TEST_SECRET");
        std::env::remove_var("RIVERS_TEST_ALLOWED");
    }

    // ── both mode ────────────────────────────────────────────────────

    #[tokio::test]
    async fn both_mode_stdin_and_args() {
        let dir = tempfile::tempdir().unwrap();
        // Script that reads stdin and also prints its args, combining into JSON
        let script = create_test_script(
            dir.path(),
            "both.sh",
            r#"#!/bin/sh
STDIN_DATA=$(cat)
printf '{"stdin":%s,"args":["' "$STDIN_DATA"
first=true
for arg in "$@"; do
    if [ "$first" = true ]; then
        first=false
    else
        printf '","'
    fi
    printf '%s' "$arg"
done
printf '"]}'
"#,
        );

        let global = make_global_config(dir.path());
        let cmd = make_command_config(
            script,
            InputMode::Both,
            Some(vec!["--host".into(), "{host}".into()]),
            Some("payload".into()),
        );
        let params = json!({
            "host": "example.com",
            "payload": {"data": "secret"}
        });

        let result = execute_command(&cmd, &global, &params).await.unwrap();

        // stdin should have received the payload value
        assert_eq!(result["stdin"], json!({"data": "secret"}));
        // args should have the interpolated template
        assert_eq!(result["args"], json!(["--host", "example.com"]));
    }

    // ── output overflow ──────────────────────────────────────────────

    #[tokio::test]
    async fn output_overflow_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        // Script that outputs a lot of data
        let script = create_test_script(
            dir.path(),
            "big_output.sh",
            "#!/bin/sh\nyes '{\"x\":1}' | head -n 100000\n",
        );

        let global = make_global_config(dir.path());
        let mut cmd = make_command_config(script, InputMode::Stdin, None, None);
        cmd.max_stdout_bytes = Some(256); // Very small limit
        let params = json!({});

        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("output exceeded limit"),
            "unexpected error: {msg}"
        );
    }

    // ── evaluate_result unit tests ───────────────────────────────────

    #[test]
    fn evaluate_success_valid_json() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0);
        let stdout = b"{\"ok\":true}";
        let stderr = b"";
        let result = evaluate_result(stdout, stderr, exit).unwrap();
        assert_eq!(result, json!({"ok": true}));
    }

    #[test]
    fn evaluate_non_zero_exit() {
        use std::os::unix::process::ExitStatusExt;
        // Exit code 1 is encoded as 0x0100 in raw status on Unix
        let exit = std::process::ExitStatus::from_raw(0x0100);
        let stdout = b"";
        let stderr = b"error message";
        let err = evaluate_result(stdout, stderr, exit).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("command failed"), "unexpected: {msg}");
        assert!(msg.contains("error message"), "unexpected: {msg}");
    }

    #[test]
    fn evaluate_empty_stdout() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0);
        let stdout = b"";
        let stderr = b"";
        let err = evaluate_result(stdout, stderr, exit).unwrap_err();
        assert!(err.to_string().contains("command produced no output"));
    }

    #[test]
    fn evaluate_invalid_json() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0);
        let stdout = b"not json";
        let stderr = b"";
        let err = evaluate_result(stdout, stderr, exit).unwrap_err();
        assert!(err.to_string().contains("command produced invalid JSON"));
    }

    #[test]
    fn evaluate_stderr_truncated_to_1024() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0x0100);
        let stdout = b"";
        // Create stderr longer than 1024 bytes
        let stderr = "x".repeat(2000);
        let err = evaluate_result(stdout, stderr.as_bytes(), exit).unwrap_err();
        let msg = err.to_string();
        // The message should be truncated — not contain all 2000 chars
        // "command failed: exit 1: " is ~24 chars, plus 1024 of stderr = ~1048
        assert!(msg.len() < 1200, "stderr should be truncated, got len {}", msg.len());
    }
}
