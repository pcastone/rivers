//! ExecDriver configuration parsing from ConnectionParams.

use std::collections::HashMap;
use std::path::PathBuf;

use rivers_driver_sdk::{ConnectionParams, DriverError};

use super::types::*;

// ── ExecConfig::parse ─────────────────────────────────────────────────

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

        // RW1.2.f: fail closed on invalid env_clear values.
        // Accept "true"/"false" case-insensitively; anything else (e.g. "yes",
        // "True", typos) is rejected with a config error rather than silently
        // inheriting the host environment.
        let env_clear = match opts.get(&format!("{prefix}env_clear")) {
            None => true, // default: clear env
            Some(s) => match s.to_ascii_lowercase().as_str() {
                "true" => true,
                "false" => false,
                other => {
                    return Err(DriverError::Connection(format!(
                        "exec driver: command '{name}' invalid env_clear value: '{other}' — expected 'true' or 'false'"
                    )));
                }
            },
        };

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

#[cfg(test)]
mod tests {
    use super::*;

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

    // ── env_clear parsing (RW1.2.f) ────────────────────────────────────

    #[test]
    fn parse_env_clear_true_lowercase() {
        let params = make_params(vec![
            ("run_as_user", "rivers"),
            ("commands.cmd.path", "/bin/cmd"),
            ("commands.cmd.sha256", "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            ("commands.cmd.env_clear", "true"),
        ]);
        let config = ExecConfig::parse(&params).unwrap();
        assert!(config.commands["cmd"].env_clear);
    }

    #[test]
    fn parse_env_clear_false_lowercase() {
        let params = make_params(vec![
            ("run_as_user", "rivers"),
            ("commands.cmd.path", "/bin/cmd"),
            ("commands.cmd.sha256", "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            ("commands.cmd.env_clear", "false"),
        ]);
        let config = ExecConfig::parse(&params).unwrap();
        assert!(!config.commands["cmd"].env_clear);
    }

    #[test]
    fn parse_env_clear_true_mixed_case() {
        let params = make_params(vec![
            ("run_as_user", "rivers"),
            ("commands.cmd.path", "/bin/cmd"),
            ("commands.cmd.sha256", "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            ("commands.cmd.env_clear", "True"),
        ]);
        let config = ExecConfig::parse(&params).unwrap();
        assert!(config.commands["cmd"].env_clear);
    }

    #[test]
    fn parse_env_clear_invalid_rejects() {
        let params = make_params(vec![
            ("run_as_user", "rivers"),
            ("commands.cmd.path", "/bin/cmd"),
            ("commands.cmd.sha256", "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
            ("commands.cmd.env_clear", "yes"),
        ]);
        let err = ExecConfig::parse(&params).unwrap_err();
        assert!(
            err.to_string().contains("invalid env_clear"),
            "expected env_clear error, got: {err}"
        );
    }

    #[test]
    fn parse_env_clear_default_is_true() {
        // When env_clear is absent, it defaults to true (fail-safe).
        let params = make_params(vec![
            ("run_as_user", "rivers"),
            ("commands.cmd.path", "/bin/cmd"),
            ("commands.cmd.sha256", "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"),
        ]);
        let config = ExecConfig::parse(&params).unwrap();
        assert!(config.commands["cmd"].env_clear);
    }
}
