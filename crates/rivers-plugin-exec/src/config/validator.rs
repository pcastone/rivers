//! ExecDriver configuration validation.

use rivers_driver_sdk::DriverError;

use super::types::*;

// ── ExecConfig::validate ──────────────────────────────────────────────

impl ExecConfig {
    /// Validate the parsed config at startup (spec section 4.2, 5.1).
    ///
    /// Returns `DriverError::Connection` on any validation failure.
    pub fn validate(&self) -> Result<(), DriverError> {
        // run_as_user must not be empty
        if self.run_as_user.is_empty() {
            return Err(DriverError::Connection(
                "exec driver: run_as_user must not be empty".into(),
            ));
        }

        // run_as_user must not be "root"
        if self.run_as_user == "root" {
            return Err(DriverError::Connection(
                "exec driver: run_as_user must not be 'root'".into(),
            ));
        }

        // run_as_user must resolve to a valid OS user (spec S4.2)
        #[cfg(unix)]
        {
            let c_user = std::ffi::CString::new(self.run_as_user.as_str()).map_err(|_| {
                DriverError::Connection(format!(
                    "exec driver: invalid run_as_user: '{}'",
                    self.run_as_user
                ))
            })?;
            let pw = unsafe { libc::getpwnam(c_user.as_ptr()) };
            if pw.is_null() {
                return Err(DriverError::Connection(format!(
                    "exec driver: run_as_user '{}' does not exist on this system",
                    self.run_as_user
                )));
            }
            let uid = unsafe { (*pw).pw_uid };
            if uid == 0 {
                return Err(DriverError::Connection(
                    "exec driver: run_as_user must not be root (UID 0)".into(),
                ));
            }
        }

        // working_directory must exist and be a directory (spec S4.2)
        if !self.working_directory.exists() {
            return Err(DriverError::Connection(format!(
                "exec driver: working_directory '{}' does not exist",
                self.working_directory.display()
            )));
        }
        if !self.working_directory.is_dir() {
            return Err(DriverError::Connection(format!(
                "exec driver: working_directory '{}' is not a directory",
                self.working_directory.display()
            )));
        }

        // Validate each command
        for (name, cmd) in &self.commands {
            // Path must be absolute
            if !cmd.path.is_absolute() {
                return Err(DriverError::Connection(format!(
                    "exec driver: command '{name}' path must be absolute, got '{}'",
                    cmd.path.display()
                )));
            }

            // Path must exist and be a file
            if !cmd.path.is_file() {
                return Err(DriverError::Connection(format!(
                    "exec driver: command '{name}' path '{}' does not exist or is not a file",
                    cmd.path.display()
                )));
            }

            // File must be executable (spec S5.1)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let meta = std::fs::metadata(&cmd.path).map_err(|e| {
                    DriverError::Connection(format!(
                        "exec driver: cannot stat {}: {e}",
                        cmd.path.display()
                    ))
                })?;
                if meta.permissions().mode() & 0o111 == 0 {
                    return Err(DriverError::Connection(format!(
                        "exec driver: command '{name}': file {} is not executable",
                        cmd.path.display()
                    )));
                }
            }

            // sha256 must be non-empty and valid hex (64 hex chars for SHA-256)
            if cmd.sha256.is_empty() {
                return Err(DriverError::Connection(format!(
                    "exec driver: command '{name}' sha256 must not be empty"
                )));
            }
            if cmd.sha256.len() != 64 || !cmd.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(DriverError::Connection(format!(
                    "exec driver: command '{name}' sha256 must be exactly 64 hex characters, got '{}'",
                    cmd.sha256
                )));
            }

            // If input_mode is Args or Both, args_template must be Some and non-empty
            if cmd.input_mode == InputMode::Args || cmd.input_mode == InputMode::Both {
                match &cmd.args_template {
                    None => {
                        return Err(DriverError::Connection(format!(
                            "exec driver: command '{name}' with input_mode '{}' requires args_template",
                            if cmd.input_mode == InputMode::Args { "args" } else { "both" }
                        )));
                    }
                    Some(template) if template.is_empty() => {
                        return Err(DriverError::Connection(format!(
                            "exec driver: command '{name}' args_template must not be empty"
                        )));
                    }
                    _ => {}
                }
            }

            // If input_mode is Both, stdin_key must be Some and non-empty
            if cmd.input_mode == InputMode::Both {
                match &cmd.stdin_key {
                    None => {
                        return Err(DriverError::Connection(format!(
                            "exec driver: command '{name}' with input_mode 'both' requires stdin_key"
                        )));
                    }
                    Some(key) if key.is_empty() => {
                        return Err(DriverError::Connection(format!(
                            "exec driver: command '{name}' stdin_key must not be empty"
                        )));
                    }
                    _ => {}
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Return a username that exists on this system and is not root.
    /// Falls back to "daemon" if "nobody" is not found.
    fn non_root_user() -> String {
        #[cfg(unix)]
        {
            for candidate in &["nobody", "daemon", "_nobody"] {
                let c = std::ffi::CString::new(*candidate).unwrap();
                let pw = unsafe { libc::getpwnam(c.as_ptr()) };
                if !pw.is_null() {
                    let uid = unsafe { (*pw).pw_uid };
                    if uid != 0 {
                        return candidate.to_string();
                    }
                }
            }
            // Last resort: use current user
            std::env::var("USER").unwrap_or_else(|_| "nobody".into())
        }
        #[cfg(not(unix))]
        {
            "nobody".into()
        }
    }

    fn make_valid_config() -> ExecConfig {
        let mut commands = HashMap::new();
        commands.insert(
            "echo".to_string(),
            CommandConfig {
                path: PathBuf::from("/bin/echo"),
                sha256: "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
                    .to_string(),
                input_mode: InputMode::Stdin,
                args_template: None,
                stdin_key: None,
                args_schema: None,
                timeout_ms: None,
                max_stdout_bytes: None,
                max_concurrent: None,
                integrity_check: None,
                env_clear: true,
                env_allow: Vec::new(),
                env_set: HashMap::new(),
            },
        );
        ExecConfig {
            run_as_user: non_root_user(),
            working_directory: PathBuf::from("/tmp"),
            default_timeout_ms: 30000,
            max_stdout_bytes: 5_242_880,
            max_concurrent: 10,
            integrity_check: IntegrityMode::EachTime,
            commands,
        }
    }

    #[test]
    fn validate_valid_config() {
        let config = make_valid_config();
        config.validate().unwrap();
    }

    #[test]
    fn validate_empty_run_as_user() {
        let mut config = make_valid_config();
        config.run_as_user = String::new();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("run_as_user must not be empty"));
    }

    #[test]
    fn validate_root_run_as_user() {
        let mut config = make_valid_config();
        config.run_as_user = "root".into();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must not be 'root'"));
    }

    #[test]
    fn validate_non_absolute_path() {
        let mut config = make_valid_config();
        config.commands.get_mut("echo").unwrap().path = PathBuf::from("relative/path");
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must be absolute"));
    }

    #[test]
    fn validate_nonexistent_path() {
        let mut config = make_valid_config();
        config.commands.get_mut("echo").unwrap().path =
            PathBuf::from("/nonexistent/path/to/binary");
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn validate_empty_sha256() {
        let mut config = make_valid_config();
        config.commands.get_mut("echo").unwrap().sha256 = String::new();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("sha256 must not be empty"));
    }

    #[test]
    fn validate_invalid_sha256_hex() {
        let mut config = make_valid_config();
        config.commands.get_mut("echo").unwrap().sha256 = "not-valid-hex".into();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("64 hex characters"));
    }

    #[test]
    fn validate_sha256_wrong_length() {
        let mut config = make_valid_config();
        config.commands.get_mut("echo").unwrap().sha256 = "abcdef".into();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("64 hex characters"));
    }

    #[test]
    fn validate_args_mode_missing_template() {
        let mut config = make_valid_config();
        let cmd = config.commands.get_mut("echo").unwrap();
        cmd.input_mode = InputMode::Args;
        cmd.args_template = None;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("requires args_template"));
    }

    #[test]
    fn validate_args_mode_empty_template() {
        let mut config = make_valid_config();
        let cmd = config.commands.get_mut("echo").unwrap();
        cmd.input_mode = InputMode::Args;
        cmd.args_template = Some(Vec::new());
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("args_template must not be empty"));
    }

    #[test]
    fn validate_both_mode_missing_stdin_key() {
        let mut config = make_valid_config();
        let cmd = config.commands.get_mut("echo").unwrap();
        cmd.input_mode = InputMode::Both;
        cmd.args_template = Some(vec!["--flag".into()]);
        cmd.stdin_key = None;
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("requires stdin_key"));
    }

    #[test]
    fn validate_both_mode_empty_stdin_key() {
        let mut config = make_valid_config();
        let cmd = config.commands.get_mut("echo").unwrap();
        cmd.input_mode = InputMode::Both;
        cmd.args_template = Some(vec!["--flag".into()]);
        cmd.stdin_key = Some(String::new());
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("stdin_key must not be empty"));
    }

    // ── Gap fix tests ─────────────────────────────────────────────────

    #[test]
    #[cfg(unix)]
    fn validate_run_as_user_not_found() {
        let mut config = make_valid_config();
        config.run_as_user = "nonexistent_user_xyz_12345".into();
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("does not exist on this system"),
            "unexpected: {}",
            err
        );
    }

    #[test]
    fn validate_working_directory_not_exists() {
        let mut config = make_valid_config();
        config.working_directory = PathBuf::from("/nonexistent/dir/xyz");
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("working_directory")
                && err.to_string().contains("does not exist"),
            "unexpected: {}",
            err
        );
    }

    #[test]
    fn validate_working_directory_not_a_dir() {
        let mut config = make_valid_config();
        // /bin/echo exists but is a file, not a directory
        config.working_directory = PathBuf::from("/bin/echo");
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("is not a directory"),
            "unexpected: {}",
            err
        );
    }

    #[test]
    #[cfg(unix)]
    fn validate_executable_permission_check() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("not_exec.sh");
        std::fs::write(&script_path, "#!/bin/sh\necho ok\n").unwrap();
        // Set permissions to 0o644 (not executable)
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let mut config = make_valid_config();
        config.working_directory = dir.path().to_path_buf();
        let cmd = config.commands.get_mut("echo").unwrap();
        cmd.path = script_path;
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string().contains("is not executable"),
            "unexpected: {}",
            err
        );
    }
}
