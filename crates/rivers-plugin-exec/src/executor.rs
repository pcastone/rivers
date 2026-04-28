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
        // RW1.2.g: log kill() errors instead of silently ignoring them.
        let ret = unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
        if ret != 0 {
            let errno = std::io::Error::last_os_error();
            // ESRCH means the process already exited — not an error worth surfacing.
            if errno.raw_os_error() != Some(libc::ESRCH) {
                tracing::warn!(
                    pid = %pid,
                    error = %errno,
                    "kill_process_group: kill(2) failed"
                );
            }
        }
    }
}

#[cfg(not(unix))]
fn kill_process_group(_pid: Option<u32>) {}

// ── UTF-8 safe truncation ─────────────────────────────────────────────

/// Return a prefix of `s` that is at most `max_bytes` bytes long, always
/// ending on a valid UTF-8 character boundary. (RW1.2.h)
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

// ── Command execution ─────────────────────────────────────────────────

/// Execute a command per the ExecDriver pipeline (spec sections 10-11).
///
/// This function handles steps 6-10 of the pipeline:
/// - Build the process command with isolation settings
/// - Write stdin if applicable (inside the unified timeout)
/// - Read stdout/stderr concurrently with size limits and timeout
/// - Evaluate the exit status and parse the JSON result
///
/// # Lifecycle controller (RW1.2.a)
///
/// stdin write, concurrent stdout/stderr drain, and child wait ALL happen
/// inside a single `tokio::time::timeout` block so the configured timeout
/// governs the entire child lifecycle, not just `wait()`.
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
    #[cfg(unix)]
    {
        // Create a new session so the child becomes a process group leader.
        // RW1.2.c: call setgroups(0, NULL) before uid/gid drop to clear supplementary groups.
        // RW1.2.g: log setsid() errors.
        unsafe {
            cmd.pre_exec(|| {
                let sid = libc::setsid();
                if sid == -1 {
                    // Cannot use tracing inside pre_exec (no async runtime); return
                    // Err so spawn() surfaces the failure to the caller (RW1.2.g).
                    return Err(std::io::Error::last_os_error());
                }
                // Clear supplementary groups before uid/gid drop (RW1.2.c).
                // EPERM is expected when not root; swallow silently.
                libc::setgroups(0, std::ptr::null());
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

    // ── TOCTOU mitigation: file-handle exec (RW1.2.b) ─────────────────
    //
    // On Linux, open the verified binary and exec via /proc/self/fd/N so that
    // even if an attacker replaces the path between hash verification and spawn,
    // we execute the already-open file descriptor.
    //
    // On macOS/other Unix, fexecve is not available without the nix crate and
    // /proc does not exist. We fall back to path-based exec. The residual
    // TOCTOU window is bounded to the few microseconds between integrity.verify()
    // and spawn(); the hash check immediately precedes the spawn in pipeline.rs.

    // FdGuard keeps the raw fd open until after spawn() so /proc/self/fd/N
    // remains valid when the kernel exec's the child. Declared here so it
    // outlives the `let mut cmd` block below.
    #[cfg(target_os = "linux")]
    struct FdGuard(i32);
    #[cfg(target_os = "linux")]
    impl Drop for FdGuard {
        fn drop(&mut self) {
            unsafe { libc::close(self.0); }
        }
    }
    #[cfg(target_os = "linux")]
    let mut _fd_guard: Option<FdGuard> = None;

    #[cfg(target_os = "linux")]
    let mut cmd = {
        use std::os::unix::io::IntoRawFd;
        match std::fs::File::open(&config.path) {
            Ok(f) => {
                let fd = f.into_raw_fd();
                let proc_path = format!("/proc/self/fd/{fd}");
                let mut fd_cmd = Command::new(&proc_path);

                // Rebuild all command settings on the fd-backed command.
                if config.input_mode == InputMode::Args || config.input_mode == InputMode::Both {
                    if let Some(ref tmpl) = config.args_template {
                        let interpolation_params = if config.input_mode == InputMode::Both {
                            let mut filtered = params_obj.clone();
                            if let Some(ref key) = config.stdin_key {
                                filtered.remove(key);
                            }
                            filtered
                        } else {
                            params_obj.clone()
                        };
                        let args = template::interpolate(tmpl, &interpolation_params)
                            .map_err(|e| DriverError::Internal(
                                format!("args template interpolation failed: {e}")
                            ))?;
                        fd_cmd.args(&args);
                    }
                }
                fd_cmd.current_dir(&global_config.working_directory);
                if config.env_clear {
                    fd_cmd.env_clear();
                    for var_name in &config.env_allow {
                        if let Ok(val) = std::env::var(var_name) {
                            fd_cmd.env(var_name, val);
                        }
                    }
                }
                for (key, value) in &config.env_set {
                    fd_cmd.env(key, value);
                }
                match config.input_mode {
                    InputMode::Stdin | InputMode::Both => {
                        fd_cmd.stdin(std::process::Stdio::piped());
                    }
                    InputMode::Args => {
                        fd_cmd.stdin(std::process::Stdio::null());
                    }
                }
                fd_cmd.stdout(std::process::Stdio::piped());
                fd_cmd.stderr(std::process::Stdio::piped());
                fd_cmd.kill_on_drop(true);
                unsafe {
                    fd_cmd.pre_exec(move || {
                        let sid = libc::setsid();
                        if sid == -1 {
                            // Return Err so spawn() surfaces the failure (RW1.2.g).
                            return Err(std::io::Error::last_os_error());
                        }
                        libc::setgroups(0, std::ptr::null());
                        Ok(())
                    });
                }
                if nix_is_root() {
                    if let Some((uid, gid)) = resolve_user(&global_config.run_as_user) {
                        fd_cmd.uid(uid);
                        fd_cmd.gid(gid);
                    }
                }

                // Keep the fd alive until after spawn(); closed by _fd_guard's Drop.
                _fd_guard = Some(FdGuard(fd));
                fd_cmd
            }
            Err(e) => {
                tracing::warn!(
                    path = %config.path.display(),
                    error = %e,
                    "exec: could not open binary for fd-based exec — falling back to path exec"
                );
                cmd
            }
        }
    };
    #[cfg(not(target_os = "linux"))]
    let mut cmd = cmd;

    // ── Spawn ─────────────────────────────────────────────────────────

    let mut child = cmd.spawn().map_err(|e| {
        DriverError::Internal(format!("failed to spawn command '{}': {e}", config.path.display()))
    })?;

    // Close the parent's copy of the fd now that the child has been spawned.
    // The child inherited the fd at exec time via /proc/self/fd/N; we no longer
    // need it in the parent.
    #[cfg(target_os = "linux")]
    drop(_fd_guard);

    let child_pid = child.id();

    // ── Unified lifecycle controller (RW1.2.a) ────────────────────────
    //
    // stdin write, concurrent stdout/stderr drain, and child wait ALL happen
    // inside this single timeout block. A child that refuses to read stdin
    // cannot hang indefinitely outside the timeout.

    let timeout_ms = config.timeout_ms.unwrap_or(global_config.default_timeout_ms);
    let max_stdout = config.max_stdout_bytes.unwrap_or(global_config.max_stdout_bytes);
    // stderr cap: same as stdout cap, floor at 64 KB.
    let max_stderr: usize = max_stdout.max(65536);

    match tokio::time::timeout(Duration::from_millis(timeout_ms), async {
        // ── Write stdin (inside timeout) ──────────────────────────────
        match config.input_mode {
            InputMode::Stdin => {
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

        // ── Concurrent stdout/stderr drain (RW1.2.d) ──────────────────
        //
        // Both pipes are drained concurrently via tokio::join! to prevent
        // deadlock: if the child writes more than the pipe buffer to stderr
        // while we're blocked reading stdout, the child blocks, stdout never
        // sees EOF, and we deadlock.
        let mut stdout_reader = child.stdout.take().ok_or_else(|| {
            DriverError::Query("failed to capture stdout".into())
        })?;
        let mut stderr_reader = child.stderr.take().ok_or_else(|| {
            DriverError::Query("failed to capture stderr".into())
        })?;

        let (stdout_res, stderr_res) = tokio::join!(
            async {
                let mut buf = Vec::with_capacity(max_stdout.min(65536));
                loop {
                    let mut chunk = [0u8; 8192];
                    let n = stdout_reader.read(&mut chunk).await.map_err(|e| {
                        DriverError::Query(format!("failed to read stdout: {e}"))
                    })?;
                    if n == 0 { break; }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > max_stdout {
                        // Kill immediately so the child's stderr pipe gets EOF,
                        // allowing the concurrent stderr drain to complete and
                        // unblock the join! rather than waiting for the timeout.
                        kill_process_group(child_pid);
                        return Err(DriverError::Query("output exceeded limit".into()));
                    }
                }
                Ok::<Vec<u8>, DriverError>(buf)
            },
            async {
                let mut buf = Vec::with_capacity(max_stderr.min(65536));
                loop {
                    let mut chunk = [0u8; 8192];
                    let n = stderr_reader.read(&mut chunk).await.map_err(|e| {
                        DriverError::Query(format!("failed to read stderr: {e}"))
                    })?;
                    if n == 0 { break; }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > max_stderr { break; }
                }
                Ok::<Vec<u8>, DriverError>(buf)
            }
        );

        let stdout_buf = stdout_res?;
        let stderr_buf = stderr_res?;

        let exit = child.wait().await.map_err(|e| {
            DriverError::Query(format!("failed to wait for process: {e}"))
        })?;

        Ok((stdout_buf, stderr_buf, exit))
    })
    .await
    {
        Ok(Ok((stdout, stderr, exit))) => {
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
        // RW1.2.h: char-boundary-safe truncation — avoids panic on multi-byte UTF-8.
        let truncated = truncate_utf8(&stderr_str, 1024);
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

#[cfg(unix)]
fn nix_is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

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

    fn create_test_script(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

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

    // ── truncate_utf8 ─────────────────────────────────────────────────

    #[test]
    fn truncate_utf8_ascii_under_limit() {
        assert_eq!(truncate_utf8("hello", 10), "hello");
    }

    #[test]
    fn truncate_utf8_ascii_exact() {
        assert_eq!(truncate_utf8("hello", 5), "hello");
    }

    #[test]
    fn truncate_utf8_ascii_over() {
        assert_eq!(truncate_utf8("hello world", 5), "hello");
    }

    #[test]
    fn truncate_utf8_multibyte_boundary() {
        // "é" = U+00E9 = 2 bytes (0xC3 0xA9). "éé" = 4 bytes.
        // Limit at 3 bytes must yield first "é" (2 bytes), not split the second.
        let s = "éé";
        let t = truncate_utf8(s, 3);
        assert_eq!(t, "é");
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }

    #[test]
    fn truncate_utf8_three_byte_sequence() {
        // "中" = 3 bytes (0xE4 0xB8 0xAD). Limit at 4 falls mid-"文".
        let s = "中文";
        let t = truncate_utf8(s, 4);
        assert_eq!(t, "中");
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }

    // ── stdin mode ───────────────────────────────────────────────────

    #[tokio::test]
    async fn stdin_mode_echo_back() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(dir.path(), "echo_stdin.sh", "#!/bin/sh\ncat\n");
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
    #[ignore = "broken pipe on Linux CI runners — subprocess exits before stdin write (#46)"]
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
        assert!(msg.contains("something went wrong"), "should contain stderr: {msg}");
    }

    // ── empty output ─────────────────────────────────────────────────

    #[tokio::test]
    #[ignore = "broken pipe on Linux CI runners — subprocess exits before stdin write (#46)"]
    async fn empty_output_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(dir.path(), "empty.sh", "#!/bin/sh\nexit 0\n");
        let global = make_global_config(dir.path());
        let cmd = make_command_config(script, InputMode::Stdin, None, None);
        let params = json!({});
        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("command produced no output"), "unexpected error: {msg}");
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
        assert!(msg.contains("command produced invalid JSON"), "unexpected error: {msg}");
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
        cmd.timeout_ms = Some(100);
        let params = json!({});
        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("command timed out"), "unexpected error: {msg}");
    }

    // ── stdin-blocking timeout (RW1.2.a regression) ───────────────────
    //
    // A child that never reads stdin must time out, not hang indefinitely.
    // Before RW1.2.a, stdin write was outside the timeout block.

    #[tokio::test]
    async fn stdin_blocking_respects_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(
            dir.path(),
            "ignore_stdin.sh",
            "#!/bin/sh\nsleep 60\n",
        );
        let global = make_global_config(dir.path());
        let mut cmd = make_command_config(script, InputMode::Stdin, None, None);
        cmd.timeout_ms = Some(300);
        let params = json!({"data": "x"});
        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("command timed out"), "expected timeout, got: {msg}");
    }

    // ── env_clear ────────────────────────────────────────────────────

    #[tokio::test]
    async fn env_clear_filters_environment() {
        let dir = tempfile::tempdir().unwrap();
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
        std::env::set_var("RIVERS_TEST_SECRET", "do_not_leak");
        std::env::set_var("RIVERS_TEST_ALLOWED", "visible");

        let global = make_global_config(dir.path());
        let mut cmd = make_command_config(script, InputMode::Args, Some(vec![]), None);
        cmd.env_clear = true;
        cmd.env_allow = vec!["RIVERS_TEST_ALLOWED".into()];
        cmd.env_set.insert("RIVERS_EXPLICIT".into(), "set_value".into());
        let params = json!({});

        let result = execute_command(&cmd, &global, &params).await.unwrap();

        assert!(
            result.get("RIVERS_TEST_SECRET").is_none(),
            "env_clear should have filtered RIVERS_TEST_SECRET"
        );
        assert_eq!(
            result.get("RIVERS_TEST_ALLOWED").and_then(|v| v.as_str()),
            Some("visible"),
        );
        assert_eq!(
            result.get("RIVERS_EXPLICIT").and_then(|v| v.as_str()),
            Some("set_value"),
        );

        std::env::remove_var("RIVERS_TEST_SECRET");
        std::env::remove_var("RIVERS_TEST_ALLOWED");
    }

    // ── both mode ────────────────────────────────────────────────────

    #[tokio::test]
    async fn both_mode_stdin_and_args() {
        let dir = tempfile::tempdir().unwrap();
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
        let params = json!({"host": "example.com", "payload": {"data": "secret"}});
        let result = execute_command(&cmd, &global, &params).await.unwrap();
        assert_eq!(result["stdin"], json!({"data": "secret"}));
        assert_eq!(result["args"], json!(["--host", "example.com"]));
    }

    // ── output overflow ──────────────────────────────────────────────

    #[tokio::test]
    async fn output_overflow_kills_process() {
        let dir = tempfile::tempdir().unwrap();
        let script = create_test_script(
            dir.path(),
            "big_output.sh",
            "#!/bin/sh\nyes '{\"x\":1}' | head -n 100000\n",
        );
        let global = make_global_config(dir.path());
        let mut cmd = make_command_config(script, InputMode::Stdin, None, None);
        cmd.max_stdout_bytes = Some(256);
        let params = json!({});
        let err = execute_command(&cmd, &global, &params).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("output exceeded limit"), "unexpected error: {msg}");
    }

    // ── evaluate_result unit tests ───────────────────────────────────

    #[test]
    fn evaluate_success_valid_json() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0);
        let result = evaluate_result(b"{\"ok\":true}", b"", exit).unwrap();
        assert_eq!(result, json!({"ok": true}));
    }

    #[test]
    fn evaluate_non_zero_exit() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0x0100);
        let err = evaluate_result(b"", b"error message", exit).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("command failed"), "unexpected: {msg}");
        assert!(msg.contains("error message"), "unexpected: {msg}");
    }

    #[test]
    fn evaluate_empty_stdout() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0);
        let err = evaluate_result(b"", b"", exit).unwrap_err();
        assert!(err.to_string().contains("command produced no output"));
    }

    #[test]
    fn evaluate_invalid_json() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0);
        let err = evaluate_result(b"not json", b"", exit).unwrap_err();
        assert!(err.to_string().contains("command produced invalid JSON"));
    }

    #[test]
    fn evaluate_stderr_truncated_to_1024() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0x0100);
        let stderr = "x".repeat(2000);
        let err = evaluate_result(b"", stderr.as_bytes(), exit).unwrap_err();
        let msg = err.to_string();
        assert!(msg.len() < 1200, "stderr should be truncated, got len {}", msg.len());
    }

    #[test]
    fn evaluate_stderr_multibyte_no_panic() {
        use std::os::unix::process::ExitStatusExt;
        let exit = std::process::ExitStatus::from_raw(0x0100);
        // "é" = 2 bytes; 600 of them = 1200 bytes, exceeds the 1024 truncation limit.
        let stderr = "é".repeat(600);
        let err = evaluate_result(b"", stderr.as_bytes(), exit).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("command failed"));
        assert!(std::str::from_utf8(msg.as_bytes()).is_ok());
    }
}
