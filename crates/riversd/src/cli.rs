//! CLI argument parsing for riversd.
//!
//! Per `rivers-application-spec.md` §12.
//!
//! Commands:
//! - `riversd` / `riversd serve` — start the server (default)
//! - `riversd version`           — show version info
//! - `riversd help`              — show help
//!
//! See also: `riversctl start` (launch riversd), `riversctl doctor` (health checks),
//!           `riverpackage preflight` (bundle validation), `rivers-lockbox` (secrets).

use std::path::PathBuf;

// ── CLI Arguments ───────────────────────────────────────────────

/// Parsed CLI arguments.
#[derive(Debug, Clone)]
pub struct CliArgs {
    pub command: CliCommand,
    pub config_path: Option<PathBuf>,
    pub log_level: Option<String>,
    pub no_admin_auth: bool,
    pub no_ssl: bool,
    pub no_ssl_port: Option<u16>,
}

/// CLI command variants.
#[derive(Debug, Clone)]
pub enum CliCommand {
    /// Start the server (default).
    Serve,
    /// Show version info.
    Version,
    /// Show help.
    Help,
}

impl Default for CliArgs {
    fn default() -> Self {
        Self {
            command: CliCommand::Serve,
            config_path: None,
            log_level: None,
            no_admin_auth: false,
            no_ssl: false,
            no_ssl_port: None,
        }
    }
}

/// Parse CLI arguments from an iterator.
pub fn parse_args<I, S>(args: I) -> Result<CliArgs, CliError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut cli = CliArgs::default();
    let mut args = args.into_iter().peekable();

    // Skip binary name
    args.next();

    while let Some(arg) = args.next() {
        let arg = arg.as_ref();
        match arg {
            "--config" | "-c" => {
                let path = args
                    .next()
                    .ok_or_else(|| CliError::MissingValue("--config".to_string()))?;
                cli.config_path = Some(PathBuf::from(path.as_ref()));
            }
            "--log-level" | "-l" => {
                let level = args
                    .next()
                    .ok_or_else(|| CliError::MissingValue("--log-level".to_string()))?;
                cli.log_level = Some(level.as_ref().to_string());
            }
            "--no-admin-auth" => {
                cli.no_admin_auth = true;
            }
            "--no-ssl" => {
                cli.no_ssl = true;
            }
            "--port" => {
                if !cli.no_ssl {
                    return Err(CliError::InvalidUsage("--port is only valid with --no-ssl".to_string()));
                }
                let port_str = args
                    .next()
                    .ok_or_else(|| CliError::MissingValue("--port".to_string()))?;
                cli.no_ssl_port = Some(
                    port_str
                        .as_ref()
                        .parse::<u16>()
                        .map_err(|_| CliError::InvalidUsage("--port requires a valid port number (1–65535)".to_string()))?,
                );
            }
            "--version" | "-V" => {
                cli.command = CliCommand::Version;
            }
            "--help" | "-h" => {
                cli.command = CliCommand::Help;
            }
            "serve" => {
                cli.command = CliCommand::Serve;
            }
            "help" => {
                cli.command = CliCommand::Help;
            }
            "version" => {
                cli.command = CliCommand::Version;
            }
            other if other.starts_with('-') => {
                return Err(CliError::UnknownFlag(other.to_string()));
            }
            other => {
                return Err(CliError::UnknownCommand(other.to_string()));
            }
        }
    }

    Ok(cli)
}

/// Generate version string.
pub fn version_string() -> String {
    format!(
        "riversd {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH
    )
}

/// Generate help text.
pub fn help_text() -> String {
    format!(
        "{}\n\n\
        USAGE:\n    \
            riversd [OPTIONS] [COMMAND]\n\n\
        COMMANDS:\n    \
            serve       Start the server (default)\n    \
            version     Show version info\n    \
            help        Show this help\n\n\
        OPTIONS:\n    \
            -c, --config <PATH>    Config file path (auto-discovered if omitted)\n    \
            -l, --log-level <LVL>  Log level (trace, debug, info, warn, error)\n    \
            --no-admin-auth        Disable admin API authentication\n    \
            --no-ssl               Run in plain HTTP mode (debug only; admin TLS rules unchanged)\n    \
            --port <port>          Bind port for --no-ssl mode (default: redirect_port from config)\n    \
            -V, --version          Show version\n    \
            -h, --help             Show help\n\n\
        SEE ALSO:\n    \
            riversctl start     Launch riversd with config and bundle\n    \
            riversctl doctor    Run pre-launch health checks\n    \
            riverpackage        Bundle validation and preflight\n    \
            rivers-lockbox      Secret keystore management\n",
        version_string()
    )
}

// ── Error Types ─────────────────────────────────────────────────

/// CLI errors.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("missing value for {0}")]
    MissingValue(String),

    #[error("unknown flag: {0}")]
    UnknownFlag(String),

    #[error("unknown command: {0}")]
    UnknownCommand(String),

    #[error("{0}")]
    InvalidUsage(String),
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_ssl_flag_parsed() {
        let args = ["riversd", "--no-ssl"];
        let cli = parse_args(args).unwrap();
        assert!(cli.no_ssl);
        assert!(cli.no_ssl_port.is_none());
    }

    #[test]
    fn no_ssl_with_port_parsed() {
        let args = ["riversd", "--no-ssl", "--port", "8080"];
        let cli = parse_args(args).unwrap();
        assert!(cli.no_ssl);
        assert_eq!(cli.no_ssl_port, Some(8080));
    }

    #[test]
    fn port_without_no_ssl_is_error() {
        let args = ["riversd", "--port", "8080"];
        let result = parse_args(args);
        assert!(result.is_err());
    }
}
