//! ExecDriver configuration type definitions.

use std::collections::HashMap;
use std::path::PathBuf;

use rivers_driver_sdk::DriverError;

// ── Types ──────────────────────────────────────────────────────────────

/// Global ExecDriver datasource configuration (spec section 4.1).
#[derive(Debug, Clone)]
pub struct ExecConfig {
    /// Unix username for privilege-dropped execution.
    pub run_as_user: String,
    /// Working directory for spawned processes.
    pub working_directory: PathBuf,
    /// Default timeout in milliseconds for command execution.
    pub default_timeout_ms: u64,
    /// Maximum bytes to read from stdout before truncation.
    pub max_stdout_bytes: usize,
    /// Global concurrency limit across all commands.
    pub max_concurrent: usize,
    /// Default integrity check mode for all commands.
    pub integrity_check: IntegrityMode,
    /// Named command configurations.
    pub commands: HashMap<String, CommandConfig>,
}

/// Per-command configuration (spec section 5).
#[derive(Debug, Clone)]
pub struct CommandConfig {
    /// Absolute path to the executable.
    pub path: PathBuf,
    /// Expected SHA-256 hash of the executable file.
    pub sha256: String,
    /// How parameters are delivered to the command.
    pub input_mode: InputMode,
    /// Template strings for argument interpolation (when `input_mode` is `Args` or `Both`).
    pub args_template: Option<Vec<String>>,
    /// Key whose value is sent on stdin (when `input_mode` is `Both`).
    pub stdin_key: Option<String>,
    /// Path to a JSON schema file for input validation.
    pub args_schema: Option<PathBuf>,
    /// Per-command timeout override in milliseconds.
    pub timeout_ms: Option<u64>,
    /// Per-command stdout byte limit override.
    pub max_stdout_bytes: Option<usize>,
    /// Per-command concurrency limit override.
    pub max_concurrent: Option<usize>,
    /// Per-command integrity check mode override.
    pub integrity_check: Option<IntegrityMode>,
    /// Whether to clear the environment before spawning.
    pub env_clear: bool,
    /// Environment variables to pass through when `env_clear` is true.
    pub env_allow: Vec<String>,
    /// Environment variables to set explicitly.
    pub env_set: HashMap<String, String>,
}

/// Integrity check mode (spec section 6.1).
#[derive(Debug, Clone)]
pub enum IntegrityMode {
    /// Hash every invocation.
    EachTime,
    /// Hash once at startup only.
    StartupOnly,
    /// Hash every N invocations.
    Every(u64),
}

/// Input delivery mode (spec section 7).
#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    /// Parameters delivered on stdin as JSON.
    Stdin,
    /// Parameters interpolated into argument template.
    Args,
    /// Parameters split: stdin_key value on stdin, rest on args.
    Both,
}

impl IntegrityMode {
    /// Parse an integrity mode string.
    ///
    /// Accepted values: `"each_time"`, `"startup_only"`, `"every:N"`.
    pub fn parse(s: &str) -> Result<IntegrityMode, DriverError> {
        match s {
            "each_time" => Ok(IntegrityMode::EachTime),
            "startup_only" => Ok(IntegrityMode::StartupOnly),
            other => {
                if let Some(n_str) = other.strip_prefix("every:") {
                    let n: u64 = n_str.parse().map_err(|_| {
                        DriverError::Connection(format!(
                            "invalid integrity_check: '{other}' — expected 'every:N' where N is a positive integer"
                        ))
                    })?;
                    if n == 0 {
                        return Err(DriverError::Connection(format!(
                            "invalid integrity_check: '{other}' — N must be positive"
                        )));
                    }
                    Ok(IntegrityMode::Every(n))
                } else {
                    Err(DriverError::Connection(format!(
                        "invalid integrity_check: '{other}' — expected 'each_time', 'startup_only', or 'every:N'"
                    )))
                }
            }
        }
    }
}

impl InputMode {
    /// Parse an input mode string.
    ///
    /// Accepted values: `"stdin"`, `"args"`, `"both"`.
    pub fn parse(s: &str) -> Result<InputMode, DriverError> {
        match s {
            "stdin" => Ok(InputMode::Stdin),
            "args" => Ok(InputMode::Args),
            "both" => Ok(InputMode::Both),
            other => Err(DriverError::Connection(format!(
                "invalid input_mode: '{other}' — expected 'stdin', 'args', or 'both'"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── IntegrityMode parsing ──────────────────────────────────────────

    #[test]
    fn parse_integrity_each_time() {
        match IntegrityMode::parse("each_time").unwrap() {
            IntegrityMode::EachTime => {}
            other => panic!("expected EachTime, got {other:?}"),
        }
    }

    #[test]
    fn parse_integrity_startup_only() {
        match IntegrityMode::parse("startup_only").unwrap() {
            IntegrityMode::StartupOnly => {}
            other => panic!("expected StartupOnly, got {other:?}"),
        }
    }

    #[test]
    fn parse_integrity_every_50() {
        match IntegrityMode::parse("every:50").unwrap() {
            IntegrityMode::Every(50) => {}
            other => panic!("expected Every(50), got {other:?}"),
        }
    }

    #[test]
    fn parse_integrity_invalid() {
        let err = IntegrityMode::parse("bogus").unwrap_err();
        assert!(err.to_string().contains("invalid integrity_check"));
    }

    #[test]
    fn parse_integrity_every_zero() {
        let err = IntegrityMode::parse("every:0").unwrap_err();
        assert!(err.to_string().contains("must be positive"));
    }

    #[test]
    fn parse_integrity_every_bad_number() {
        let err = IntegrityMode::parse("every:abc").unwrap_err();
        assert!(err.to_string().contains("invalid integrity_check"));
    }

    // ── InputMode parsing ──────────────────────────────────────────────

    #[test]
    fn parse_input_stdin() {
        assert_eq!(InputMode::parse("stdin").unwrap(), InputMode::Stdin);
    }

    #[test]
    fn parse_input_args() {
        assert_eq!(InputMode::parse("args").unwrap(), InputMode::Args);
    }

    #[test]
    fn parse_input_both() {
        assert_eq!(InputMode::parse("both").unwrap(), InputMode::Both);
    }

    #[test]
    fn parse_input_invalid() {
        let err = InputMode::parse("pipe").unwrap_err();
        assert!(err.to_string().contains("invalid input_mode"));
    }
}
