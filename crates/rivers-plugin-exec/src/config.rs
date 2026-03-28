//! ExecDriver configuration types and parsing.
//!
//! Configuration is extracted from `ConnectionParams.options`, which comes from
//! the TOML datasource `extra` map. Nested command configs arrive as flattened
//! dot-separated keys (e.g. `commands.network_scan.path`).

use std::collections::HashMap;
use std::path::PathBuf;

use rivers_driver_sdk::{ConnectionParams, DriverError};

// ── Types ──────────────────────────────────────────────────────────────

/// Global ExecDriver datasource configuration (spec section 4.1).
#[derive(Debug, Clone)]
pub struct ExecConfig {
    pub run_as_user: String,
    pub working_directory: PathBuf,
    pub default_timeout_ms: u64,
    pub max_stdout_bytes: usize,
    pub max_concurrent: usize,
    pub integrity_check: IntegrityMode,
    pub commands: HashMap<String, CommandConfig>,
}

/// Per-command configuration (spec section 5).
#[derive(Debug, Clone)]
pub struct CommandConfig {
    pub path: PathBuf,
    pub sha256: String,
    pub input_mode: InputMode,
    pub args_template: Option<Vec<String>>,
    pub stdin_key: Option<String>,
    pub args_schema: Option<PathBuf>,
    pub timeout_ms: Option<u64>,
    pub max_stdout_bytes: Option<usize>,
    pub max_concurrent: Option<usize>,
    pub integrity_check: Option<IntegrityMode>,
    pub env_clear: bool,
    pub env_allow: Vec<String>,
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

// ── Parsing ────────────────────────────────────────────────────────────

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

impl ExecConfig {
    /// Parse an `ExecConfig` from `ConnectionParams.options`.
    ///
    /// The TOML datasource config under `[datasources.*.extra]` is flattened
    /// into `ConnectionParams.options` as string key-value pairs. Nested command
    /// configs arrive as `commands.<name>.<field>` keys.
    pub fn parse(params: &ConnectionParams) -> Result<ExecConfig, DriverError> {
        let opts = &params.options;

        let run_as_user = opts
            .get("run_as_user")
            .ok_or_else(|| {
                DriverError::Connection("exec driver: missing required option 'run_as_user'".into())
            })?
            .clone();

        let working_directory = PathBuf::from(
            opts.get("working_directory")
                .map(|s| s.as_str())
                .unwrap_or("/tmp"),
        );

        let default_timeout_ms = parse_u64_opt(opts, "default_timeout_ms", 30000)?;
        let max_stdout_bytes = parse_usize_opt(opts, "max_stdout_bytes", 5_242_880)?;
        let max_concurrent = parse_usize_opt(opts, "max_concurrent", 10)?;

        let integrity_check = match opts.get("integrity_check") {
            Some(s) => IntegrityMode::parse(s)?,
            None => IntegrityMode::EachTime,
        };

        let commands = parse_commands(opts)?;

        Ok(ExecConfig {
            run_as_user,
            working_directory,
            default_timeout_ms,
            max_stdout_bytes,
            max_concurrent,
            integrity_check,
            commands,
        })
    }

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

// ── Helpers ────────────────────────────────────────────────────────────

/// Parse a u64 option with a default value.
fn parse_u64_opt(
    opts: &HashMap<String, String>,
    key: &str,
    default: u64,
) -> Result<u64, DriverError> {
    match opts.get(key) {
        Some(s) => s.parse::<u64>().map_err(|_| {
            DriverError::Connection(format!("exec driver: invalid {key} value: '{s}'"))
        }),
        None => Ok(default),
    }
}

/// Parse a usize option with a default value.
fn parse_usize_opt(
    opts: &HashMap<String, String>,
    key: &str,
    default: usize,
) -> Result<usize, DriverError> {
    match opts.get(key) {
        Some(s) => s.parse::<usize>().map_err(|_| {
            DriverError::Connection(format!("exec driver: invalid {key} value: '{s}'"))
        }),
        None => Ok(default),
    }
}

/// Parse commands from flattened `commands.<name>.<field>` keys.
///
/// Example keys:
/// ```text
/// commands.network_scan.path = "/usr/lib/rivers/scripts/netscan.py"
/// commands.network_scan.sha256 = "a1b2..."
/// commands.network_scan.input_mode = "stdin"
/// commands.network_scan.args_template.0 = "--cidr"
/// commands.network_scan.args_template.1 = "{cidr}"
/// commands.network_scan.env_allow.0 = "PATH"
/// commands.network_scan.env_set.HOME = "/tmp"
/// ```
fn parse_commands(
    opts: &HashMap<String, String>,
) -> Result<HashMap<String, CommandConfig>, DriverError> {
    // First pass: collect command names
    let mut command_names = std::collections::HashSet::new();
    for key in opts.keys() {
        if let Some(rest) = key.strip_prefix("commands.") {
            if let Some(dot_pos) = rest.find('.') {
                command_names.insert(rest[..dot_pos].to_string());
            }
        }
    }

    // Second pass: parse each command
    let mut commands = HashMap::new();
    for name in command_names {
        let prefix = format!("commands.{name}.");

        let path_str = opts.get(&format!("{prefix}path")).ok_or_else(|| {
            DriverError::Connection(format!(
                "exec driver: command '{name}' missing required field 'path'"
            ))
        })?;

        let sha256 = opts
            .get(&format!("{prefix}sha256"))
            .ok_or_else(|| {
                DriverError::Connection(format!(
                    "exec driver: command '{name}' missing required field 'sha256'"
                ))
            })?
            .clone();

        let input_mode_str = opts
            .get(&format!("{prefix}input_mode"))
            .map(|s| s.as_str())
            .unwrap_or("stdin");
        let input_mode = InputMode::parse(input_mode_str)?;

        let args_template = parse_indexed_list(opts, &format!("{prefix}args_template"));
        let args_template = if args_template.is_empty() {
            None
        } else {
            Some(args_template)
        };

        let stdin_key = opts.get(&format!("{prefix}stdin_key")).cloned();

        let args_schema = opts
            .get(&format!("{prefix}args_schema"))
            .map(PathBuf::from);

        let timeout_ms = match opts.get(&format!("{prefix}timeout_ms")) {
            Some(s) => Some(s.parse::<u64>().map_err(|_| {
                DriverError::Connection(format!(
                    "exec driver: command '{name}' invalid timeout_ms: '{s}'"
                ))
            })?),
            None => None,
        };

        let max_stdout_bytes = match opts.get(&format!("{prefix}max_stdout_bytes")) {
            Some(s) => Some(s.parse::<usize>().map_err(|_| {
                DriverError::Connection(format!(
                    "exec driver: command '{name}' invalid max_stdout_bytes: '{s}'"
                ))
            })?),
            None => None,
        };

        let max_concurrent = match opts.get(&format!("{prefix}max_concurrent")) {
            Some(s) => Some(s.parse::<usize>().map_err(|_| {
                DriverError::Connection(format!(
                    "exec driver: command '{name}' invalid max_concurrent: '{s}'"
                ))
            })?),
            None => None,
        };

        let integrity_check = match opts.get(&format!("{prefix}integrity_check")) {
            Some(s) => Some(IntegrityMode::parse(s)?),
            None => None,
        };

        let env_clear = opts
            .get(&format!("{prefix}env_clear"))
            .map(|s| s == "true")
            .unwrap_or(true);

        let env_allow = parse_indexed_list(opts, &format!("{prefix}env_allow"));

        let env_set = parse_env_set(opts, &format!("{prefix}env_set."));

        commands.insert(
            name,
            CommandConfig {
                path: PathBuf::from(path_str),
                sha256,
                input_mode,
                args_template,
                stdin_key,
                args_schema,
                timeout_ms,
                max_stdout_bytes,
                max_concurrent,
                integrity_check,
                env_clear,
                env_allow,
                env_set,
            },
        );
    }

    Ok(commands)
}

/// Parse an indexed list from flattened keys (e.g. `prefix.0`, `prefix.1`, ...).
fn parse_indexed_list(opts: &HashMap<String, String>, prefix: &str) -> Vec<String> {
    let mut items: Vec<(usize, String)> = Vec::new();
    for (key, value) in opts {
        if let Some(rest) = key.strip_prefix(prefix) {
            if let Some(idx_str) = rest.strip_prefix('.') {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    items.push((idx, value.clone()));
                }
            }
        }
    }
    items.sort_by_key(|(idx, _)| *idx);
    items.into_iter().map(|(_, v)| v).collect()
}

/// Parse env_set keys from flattened `prefix.KEY = VALUE` pairs.
fn parse_env_set(opts: &HashMap<String, String>, prefix: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for (key, value) in opts {
        if let Some(env_key) = key.strip_prefix(prefix) {
            if !env_key.is_empty() {
                env.insert(env_key.to_string(), value.clone());
            }
        }
    }
    env
}

// ── Tests ──────────────────────────────────────────────────────────────

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

    // ── ExecConfig parsing from ConnectionParams ───────────────────────

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

    #[test]
    fn parse_config_minimal() {
        let params = make_params(vec![
            ("run_as_user", "rivers"),
            ("commands.echo.path", "/bin/echo"),
            ("commands.echo.sha256", "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            ("commands.echo.input_mode", "stdin"),
        ]);
        let config = ExecConfig::parse(&params).unwrap();
        assert_eq!(config.run_as_user, "rivers");
        assert_eq!(config.working_directory, PathBuf::from("/tmp"));
        assert_eq!(config.default_timeout_ms, 30000);
        assert_eq!(config.max_stdout_bytes, 5_242_880);
        assert_eq!(config.max_concurrent, 10);
        assert_eq!(config.commands.len(), 1);
        assert!(config.commands.contains_key("echo"));

        let cmd = &config.commands["echo"];
        assert_eq!(cmd.path, PathBuf::from("/bin/echo"));
        assert_eq!(cmd.input_mode, InputMode::Stdin);
        assert!(cmd.env_clear);
    }

    #[test]
    fn parse_config_with_overrides() {
        let params = make_params(vec![
            ("run_as_user", "apprunner"),
            ("working_directory", "/var/run"),
            ("default_timeout_ms", "5000"),
            ("max_stdout_bytes", "1048576"),
            ("max_concurrent", "5"),
            ("integrity_check", "startup_only"),
            ("commands.test.path", "/usr/bin/test"),
            ("commands.test.sha256", "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        ]);
        let config = ExecConfig::parse(&params).unwrap();
        assert_eq!(config.run_as_user, "apprunner");
        assert_eq!(config.working_directory, PathBuf::from("/var/run"));
        assert_eq!(config.default_timeout_ms, 5000);
        assert_eq!(config.max_stdout_bytes, 1_048_576);
        assert_eq!(config.max_concurrent, 5);
        match config.integrity_check {
            IntegrityMode::StartupOnly => {}
            other => panic!("expected StartupOnly, got {other:?}"),
        }
    }

    #[test]
    fn parse_config_missing_run_as_user() {
        let params = make_params(vec![
            ("commands.echo.path", "/bin/echo"),
            ("commands.echo.sha256", "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234"),
        ]);
        let err = ExecConfig::parse(&params).unwrap_err();
        assert!(err.to_string().contains("run_as_user"));
    }

    #[test]
    fn parse_config_args_template() {
        let params = make_params(vec![
            ("run_as_user", "rivers"),
            ("commands.scan.path", "/usr/bin/scan"),
            ("commands.scan.sha256", "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            ("commands.scan.input_mode", "args"),
            ("commands.scan.args_template.0", "--target"),
            ("commands.scan.args_template.1", "{host}"),
            ("commands.scan.args_template.2", "--port"),
            ("commands.scan.args_template.3", "{port}"),
        ]);
        let config = ExecConfig::parse(&params).unwrap();
        let cmd = &config.commands["scan"];
        assert_eq!(cmd.input_mode, InputMode::Args);
        assert_eq!(
            cmd.args_template.as_ref().unwrap(),
            &["--target", "{host}", "--port", "{port}"]
        );
    }

    #[test]
    fn parse_config_env_set() {
        let params = make_params(vec![
            ("run_as_user", "rivers"),
            ("commands.cmd.path", "/bin/cmd"),
            ("commands.cmd.sha256", "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            ("commands.cmd.env_set.HOME", "/tmp"),
            ("commands.cmd.env_set.LANG", "en_US.UTF-8"),
        ]);
        let config = ExecConfig::parse(&params).unwrap();
        let cmd = &config.commands["cmd"];
        assert_eq!(cmd.env_set.get("HOME").unwrap(), "/tmp");
        assert_eq!(cmd.env_set.get("LANG").unwrap(), "en_US.UTF-8");
    }

    // ── Validation tests ───────────────────────────────────────────────

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
            run_as_user: "rivers".into(),
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
}
