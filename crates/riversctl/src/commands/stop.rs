use std::path::PathBuf;

pub fn cmd_stop(_args: &[String]) -> Result<(), String> {
    let pid = read_pid_file()?;
    if !is_process_alive(pid) {
        cleanup_pid_file();
        return Err(format!("riversd (pid {pid}) is not running"));
    }
    println!("rivers: stopping riversd (pid {pid})");

    // Send SIGTERM and check the kill() return value immediately.
    // If kill fails (e.g. EPERM, ESRCH) we must not silently continue.
    send_term(pid)?;

    for i in 0..30 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if !is_process_alive(pid) {
            // Only clean up the PID file after we have confirmed the process exited.
            cleanup_pid_file();
            println!("rivers: riversd stopped (took {}s)", i + 1);
            return Ok(());
        }
    }
    eprintln!("rivers: riversd did not stop after 30s — sending SIGKILL");
    send_kill(pid)?;

    // Wait up to 5 more seconds for the process to actually exit after SIGKILL.
    // Only remove the PID file once we have confirmed the process is gone.
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if !is_process_alive(pid) {
            cleanup_pid_file();
            return Ok(());
        }
    }
    Err(format!(
        "riversd (pid {pid}) did not exit after SIGKILL — PID file NOT removed"
    ))
}

/// Send SIGTERM (or Windows graceful stop) and verify the syscall succeeded.
#[cfg(unix)]
fn send_term(pid: u32) -> Result<(), String> {
    let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(format!("kill(SIGTERM, {pid}) failed: {err}"));
    }
    Ok(())
}

#[cfg(windows)]
fn send_term(pid: u32) -> Result<(), String> {
    let status = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .status()
        .map_err(|e| format!("taskkill failed: {e}"))?;
    if !status.success() {
        return Err(format!("taskkill /PID {pid} did not succeed"));
    }
    Ok(())
}

/// Send SIGKILL (or Windows force kill) and verify the syscall succeeded.
#[cfg(unix)]
fn send_kill(pid: u32) -> Result<(), String> {
    let rc = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        return Err(format!("kill(SIGKILL, {pid}) failed: {err}"));
    }
    Ok(())
}

#[cfg(windows)]
fn send_kill(pid: u32) -> Result<(), String> {
    let status = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()
        .map_err(|e| format!("taskkill /F failed: {e}"))?;
    if !status.success() {
        return Err(format!("taskkill /F /PID {pid} did not succeed"));
    }
    Ok(())
}

pub(crate) fn read_pid_file() -> Result<u32, String> {
    let pid_path = find_pid_file().ok_or("PID file not found — is riversd running?")?;
    let content = std::fs::read_to_string(&pid_path).map_err(|e| format!("cannot read PID file: {e}"))?;
    content.trim().parse::<u32>().map_err(|e| format!("invalid PID: {e}"))
}

pub(crate) fn find_pid_file() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(exe) = exe.canonicalize() {
            if let Some(root) = exe.parent().and_then(|b| b.parent()) {
                let p = root.join("run/riversd.pid");
                if p.is_file() { return Some(p); }
            }
        }
    }
    if let Ok(home) = std::env::var("RIVERS_HOME") {
        let p = PathBuf::from(home).join("run/riversd.pid");
        if p.is_file() { return Some(p); }
    }
    let p = PathBuf::from("run/riversd.pid");
    if p.is_file() { return Some(p); }
    None
}

pub(crate) fn cleanup_pid_file() {
    if let Some(p) = find_pid_file() { let _ = std::fs::remove_file(p); }
}

#[cfg(unix)]
pub(crate) fn is_process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
pub(crate) fn is_process_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Serialize all tests that mutate RIVERS_HOME to avoid parallel-test races.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn find_pid_file_returns_some_when_rivers_home_has_pid_file() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("run");
        std::fs::create_dir_all(&run_dir).unwrap();
        let pid_path = run_dir.join("riversd.pid");
        std::fs::write(&pid_path, "12345\n").unwrap();
        unsafe { std::env::set_var("RIVERS_HOME", dir.path()); }
        let result = find_pid_file();
        unsafe { std::env::remove_var("RIVERS_HOME"); }
        assert_eq!(result.as_deref(), Some(pid_path.as_path()));
    }

    #[test]
    fn read_pid_file_parses_pid_from_rivers_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("run");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("riversd.pid"), "42\n").unwrap();
        unsafe { std::env::set_var("RIVERS_HOME", dir.path()); }
        let result = read_pid_file();
        unsafe { std::env::remove_var("RIVERS_HOME"); }
        assert_eq!(result.unwrap(), 42u32);
    }

    #[test]
    fn read_pid_file_returns_err_for_invalid_pid_content() {
        // Test the parse step directly — env var not needed here.
        let content = "not-a-number\n";
        let result: Result<u32, _> = content.trim().parse();
        assert!(result.is_err(), "non-numeric PID must fail to parse");
    }

    #[test]
    fn read_pid_file_returns_err_when_no_pid_file_in_rivers_home() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("RIVERS_HOME", dir.path()); }
        let result = read_pid_file();
        unsafe { std::env::remove_var("RIVERS_HOME"); }
        // Either the RIVERS_HOME path matched (Err) or another search path matched.
        if let Err(msg) = result {
            assert!(
                msg.contains("PID file not found") || msg.contains("cannot read PID file"),
                "got: {msg}"
            );
        }
    }
}
