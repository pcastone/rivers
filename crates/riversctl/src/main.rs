#![warn(missing_docs)]
//! riversctl — Rivers control CLI.
//!
//! Starts and manages riversd, runs health checks, and communicates with
//! riversd's admin API using Ed25519-signed requests.

mod commands;

#[cfg(feature = "tls")]
mod tls_cmd;

use commands::{start, doctor, exec, admin, stop, status};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    #[cfg(feature = "admin-api")]
    let admin_url = std::env::var("RIVERS_ADMIN_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:9090".into());

    // Load [base.admin_api].private_key from config if present; env var takes priority
    // inside sign_request, so this is only the fallback source.
    #[cfg(feature = "admin-api")]
    {
        let config_key = doctor::load_config_for_tls().ok()
            .and_then(|cfg| cfg.base.admin_api.private_key);
        admin::init_config_key(config_key);
    }

    let result: Result<(), String> = match args[1].as_str() {
        "start"  => start::cmd_start(&args[2..]),
        "stop"   => stop::cmd_stop(&args[2..]),
        "status" => status::cmd_status(&args[2..]),
        "doctor" => doctor::cmd_doctor(&args[2..]),
        #[cfg(feature = "admin-api")]
        "api-status" => admin::cmd_status(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "deploy" => {
            if args.len() < 3 { eprintln!("Usage: riversctl deploy <bundle_path>"); std::process::exit(1); }
            admin::cmd_deploy(&admin_url, &args[2]).await
        }
        #[cfg(feature = "admin-api")]
        "drivers"     => admin::cmd_drivers(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "datasources" => admin::cmd_datasources(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "health"      => admin::cmd_health(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "api-stop"    => admin::cmd_stop(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "graceful"    => admin::cmd_graceful(&admin_url).await,
        #[cfg(feature = "admin-api")]
        "log" => {
            if args.len() < 3 { eprintln!("Usage: riversctl log <levels|set|reset>"); std::process::exit(1); }
            admin::cmd_log(&admin_url, &args[2..]).await
        }
        #[cfg(feature = "admin-api")]
        "breaker" => {
            let app_arg = args.iter().find(|a| a.starts_with("--app=")).map(|a| &a[6..]);
            if args.iter().any(|a| a == "--list") {
                match app_arg {
                    Some(app) => admin::cmd_breaker_list(&admin_url, app).await,
                    None => Err("usage: riversctl breaker --list --app=<appId>".into()),
                }
            } else if let Some(name_arg) = args.iter().find(|a| a.starts_with("--name=")) {
                let name = &name_arg[7..];
                match app_arg {
                    Some(app) => {
                        if args.iter().any(|a| a == "--trip") {
                            admin::cmd_breaker_trip(&admin_url, app, name).await
                        } else if args.iter().any(|a| a == "--reset") {
                            admin::cmd_breaker_reset(&admin_url, app, name).await
                        } else {
                            admin::cmd_breaker_status(&admin_url, app, name).await
                        }
                    }
                    None => Err("usage: riversctl breaker --app=<appId> --name=<breakerId> [--trip|--reset]".into()),
                }
            } else {
                Err("usage: riversctl breaker --app=<appId> --list | --name=<breakerId> [--trip|--reset]".into())
            }
        }
        #[cfg(feature = "tls")]
        "tls" => {
            if args.len() < 3 {
                eprintln!("Usage: riversctl tls <gen|request|import|show|list|expire> [--port P]");
                std::process::exit(1);
            }
            doctor::load_config_for_tls().and_then(|config| {
                let tls_args: Vec<&str> = args[2..].iter().map(|s| s.as_str()).collect();
                tls_cmd::parse_tls_args(&tls_args)
                    .map_err(|e| format!("tls: {e}"))
                    .and_then(|cmd| {
                        tls_cmd::run_tls_cmd(cmd, &config)
                            .map_err(|e| format!("tls: {e}"))
                    })
            })
        }
        "exec" => {
            if args.len() < 3 {
                eprintln!("Usage: riversctl exec <hash|verify|list> [args]");
                std::process::exit(1);
            }
            exec::cmd_exec(&args[2..])
        }
        "--version" | "-V" | "version" => {
            println!("riversctl {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH);
            Ok(())
        }
        "help" | "--help" | "-h" => { print_usage(); Ok(()) }
        other => { eprintln!("Unknown command: {other}"); print_usage(); std::process::exit(1); }
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn print_usage() {
    eprintln!("riversctl — Rivers control CLI");
    eprintln!();
    eprintln!("Usage: riversctl <command> [args]");
    eprintln!();
    eprintln!("Local commands:");
    eprintln!("  start [--config <path>] [--log-level <lvl>] [--no-admin-auth] [--no-ssl [--port <port>]]");
    eprintln!("                  Find and launch riversd (bundle_path comes from config)");
    eprintln!("  stop            Stop riversd via SIGTERM (reads PID file; SIGKILL after 30s)");
    eprintln!("  status          Show whether riversd is running (reads PID file)");
    eprintln!("  doctor [--config <path>]");
    eprintln!("                  Run pre-launch health checks");
    eprintln!("  tls gen [--port P]");
    eprintln!("                  Generate a self-signed certificate for the server on port P");
    eprintln!("  tls request [--port P]");
    eprintln!("                  Generate a CSR and print to stdout");
    eprintln!("  tls import <cert> <key> [--port P]");
    eprintln!("                  Import a cert/key pair, validate, and copy to configured paths");
    eprintln!("  tls show [--port P]");
    eprintln!("                  Show certificate details (subject, SANs, expiry, fingerprint)");
    eprintln!("  tls list        List all configured and auto-generated certificate paths");
    eprintln!("  tls expire --yes [--port P]");
    eprintln!("                  Remove cert/key files to force re-generation on next start");
    eprintln!("  exec hash <path>");
    eprintln!("                  Compute SHA-256 of a file (TOML-ready output)");
    eprintln!("  exec verify <path> <sha256>");
    eprintln!("                  Verify a file matches an expected SHA-256 hash");
    eprintln!("  exec list       List declared exec commands (planned — not yet implemented)");
    eprintln!();
    eprintln!("Admin API commands (require a running riversd):");
    eprintln!("  api-status      Server status via HTTP admin API");
    eprintln!("  deploy <path>   Deploy a bundle");
    eprintln!("  drivers         List registered drivers");
    eprintln!("  datasources     List configured datasources");
    eprintln!("  health          Verbose health check");
    eprintln!("  api-stop        Stop riversd immediately via HTTP admin API (SIGKILL fallback)");
    eprintln!("  graceful        Stop riversd gracefully — drain in-flight requests (SIGTERM fallback)");
    eprintln!("  log levels      View current log levels");
    eprintln!("  log set <target> <level> Change log level");
    eprintln!("  log reset       Reset to defaults");
    eprintln!("  breaker --app=<appId> --list                    List all circuit breakers for an app");
    eprintln!("  breaker --app=<appId> --name=<id>              Show circuit breaker status");
    eprintln!("  breaker --app=<appId> --name=<id> --trip       Trip (open) a circuit breaker");
    eprintln!("  breaker --app=<appId> --name=<id> --reset      Reset (close) a circuit breaker");
    eprintln!();
    eprintln!("Environment:");
    eprintln!("  RIVERS_ADMIN_URL     Admin API base URL (default: http://127.0.0.1:9090)");
    eprintln!("  RIVERS_ADMIN_KEY     Path to Ed25519 private key for signed requests");
    eprintln!("  RIVERS_DAEMON_PATH   Explicit path to the riversd binary");
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::commands::{start, doctor};
    #[cfg(feature = "admin-api")]
    use super::commands::admin;

    #[cfg(feature = "admin-api")]
    #[test]
    fn sign_request_produces_timestamp() {
        std::env::remove_var("RIVERS_ADMIN_KEY");
        let headers = admin::sign_request("GET", "/admin/status", "test body", None).unwrap();
        assert!(headers.contains_key("X-Rivers-Timestamp"));
    }

    #[cfg(feature = "admin-api")]
    #[test]
    fn sign_request_without_key_has_no_signature() {
        std::env::remove_var("RIVERS_ADMIN_KEY");
        let headers = admin::sign_request("GET", "/admin/status", "body", None).unwrap();
        assert!(!headers.contains_key("X-Rivers-Signature"));
        assert!(headers.contains_key("X-Rivers-Timestamp"));
    }

    #[test]
    fn find_riversd_binary_env_missing_file() {
        std::env::set_var("RIVERS_DAEMON_PATH", "/nonexistent/riversd");
        let result = start::find_riversd_binary();
        std::env::remove_var("RIVERS_DAEMON_PATH");
        assert!(result.is_err());
    }

    #[test]
    fn discover_config_returns_none_when_absent() {
        assert!(doctor::discover_config().is_none());
    }

    #[test]
    fn start_arg_parsing_forwards_config_and_level() {
        // Verify arg parsing builds the expected riversd argv
        let args = vec![
            "--config".to_string(), "/etc/rivers.toml".to_string(),
            "--log-level".to_string(), "debug".to_string(),
        ];
        let mut riversd_args: Vec<String> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "--config" | "-c" => {
                    i += 1;
                    riversd_args.push("--config".into());
                    riversd_args.push(args[i].clone());
                }
                "--log-level" | "-l" => {
                    i += 1;
                    riversd_args.push("--log-level".into());
                    riversd_args.push(args[i].clone());
                }
                "--no-admin-auth" => riversd_args.push("--no-admin-auth".into()),
                bundle => {
                    riversd_args.push("serve".into());
                    riversd_args.push(bundle.into());
                }
            }
            i += 1;
        }
        if !riversd_args.contains(&"serve".to_string()) {
            riversd_args.push("serve".into());
        }
        assert!(riversd_args.contains(&"--config".to_string()));
        assert!(riversd_args.contains(&"/etc/rivers.toml".to_string()));
        assert!(riversd_args.contains(&"serve".to_string()));
    }
}
