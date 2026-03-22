use riversd::cli::{parse_args, version_string, help_text, CliCommand, CliError};

// ── Default (serve) ─────────────────────────────────────────────

#[test]
fn default_is_serve() {
    let args = parse_args(["riversd"].iter()).unwrap();
    assert!(matches!(args.command, CliCommand::Serve));
    assert!(args.config_path.is_none());
    assert!(!args.no_admin_auth);
}

// ── Config Flag ─────────────────────────────────────────────────

#[test]
fn config_long_flag() {
    let args = parse_args(["riversd", "--config", "/etc/riversd.conf"].iter()).unwrap();
    assert_eq!(args.config_path.unwrap().to_str().unwrap(), "/etc/riversd.conf");
}

#[test]
fn config_short_flag() {
    let args = parse_args(["riversd", "-c", "/etc/riversd.conf"].iter()).unwrap();
    assert_eq!(args.config_path.unwrap().to_str().unwrap(), "/etc/riversd.conf");
}

#[test]
fn config_missing_value() {
    let result = parse_args(["riversd", "--config"].iter());
    assert!(matches!(result.unwrap_err(), CliError::MissingValue(_)));
}

// ── Log Level Flag ──────────────────────────────────────────────

#[test]
fn log_level_flag() {
    let args = parse_args(["riversd", "--log-level", "debug"].iter()).unwrap();
    assert_eq!(args.log_level.unwrap(), "debug");
}

#[test]
fn log_level_short() {
    let args = parse_args(["riversd", "-l", "trace"].iter()).unwrap();
    assert_eq!(args.log_level.unwrap(), "trace");
}

// ── No Admin Auth ───────────────────────────────────────────────

#[test]
fn no_admin_auth_flag() {
    let args = parse_args(["riversd", "--no-admin-auth"].iter()).unwrap();
    assert!(args.no_admin_auth);
}

// ── Commands ────────────────────────────────────────────────────

#[test]
fn serve_command() {
    let args = parse_args(["riversd", "serve"].iter()).unwrap();
    assert!(matches!(args.command, CliCommand::Serve));
}

#[test]
fn version_command() {
    let args = parse_args(["riversd", "version"].iter()).unwrap();
    assert!(matches!(args.command, CliCommand::Version));
}

#[test]
fn version_flag() {
    let args = parse_args(["riversd", "--version"].iter()).unwrap();
    assert!(matches!(args.command, CliCommand::Version));
}

#[test]
fn help_command() {
    let args = parse_args(["riversd", "help"].iter()).unwrap();
    assert!(matches!(args.command, CliCommand::Help));
}

#[test]
fn help_flag() {
    let args = parse_args(["riversd", "--help"].iter()).unwrap();
    assert!(matches!(args.command, CliCommand::Help));
}

// ── Error Cases ─────────────────────────────────────────────────

#[test]
fn unknown_flag() {
    let result = parse_args(["riversd", "--unknown"].iter());
    assert!(matches!(result.unwrap_err(), CliError::UnknownFlag(_)));
}

#[test]
fn unknown_command() {
    let result = parse_args(["riversd", "badcmd"].iter());
    assert!(matches!(result.unwrap_err(), CliError::UnknownCommand(_)));
}

#[test]
fn doctor_is_unknown_command() {
    // doctor moved to riversctl
    let result = parse_args(["riversd", "doctor"].iter());
    assert!(matches!(result.unwrap_err(), CliError::UnknownCommand(_)));
}

#[test]
fn preflight_is_unknown_command() {
    // preflight moved to riverpackage
    let result = parse_args(["riversd", "preflight", "/path"].iter());
    assert!(matches!(result.unwrap_err(), CliError::UnknownCommand(_)));
}

#[test]
fn lockbox_is_unknown_command() {
    // lockbox moved to rivers-lockbox
    let result = parse_args(["riversd", "lockbox", "list", "/keys"].iter());
    assert!(matches!(result.unwrap_err(), CliError::UnknownCommand(_)));
}

// ── Combined Flags ──────────────────────────────────────────────

#[test]
fn config_and_log_level() {
    let args = parse_args(["riversd", "-c", "/etc/riversd.conf", "-l", "debug"].iter()).unwrap();
    assert_eq!(args.config_path.unwrap().to_str().unwrap(), "/etc/riversd.conf");
    assert_eq!(args.log_level.unwrap(), "debug");
}

#[test]
fn flags_with_serve() {
    let args = parse_args(["riversd", "--no-admin-auth", "--config", "/cfg", "serve"].iter()).unwrap();
    assert!(args.no_admin_auth);
    assert!(args.config_path.is_some());
    assert!(matches!(args.command, CliCommand::Serve));
}

// ── Version/Help Strings ────────────────────────────────────────

#[test]
fn version_string_contains_version() {
    let v = version_string();
    assert!(v.contains("riversd"));
    assert!(v.contains("0.50.1"));
}

#[test]
fn help_text_contains_commands() {
    let h = help_text();
    assert!(h.contains("serve"));
    assert!(h.contains("--config"));
    assert!(h.contains("riversctl"));
    assert!(h.contains("riverpackage"));
    assert!(h.contains("rivers-lockbox"));
    // removed commands must not appear as commands
    assert!(!h.contains("    doctor"));
    assert!(!h.contains("    preflight"));
    assert!(!h.contains("    lockbox"));
}
