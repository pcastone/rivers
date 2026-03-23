//! riversd — Rivers runtime daemon.
//!
//! Entry point that parses CLI args, discovers config, sets up logging,
//! and starts the server.
//!
//! For health checks use `riversctl doctor`.
//! For bundle validation use `riverpackage preflight`.
//! For secret management use `rivers-lockbox`.

use std::path::{Path, PathBuf};

use rivers_runtime::rivers_core::ServerConfig;
use riversd::cli::{parse_args, CliCommand};

#[tokio::main]
async fn main() {
    // Parse CLI arguments
    let args = match parse_args(std::env::args()) {
        Ok(args) => args,
        Err(e) => {
            eprintln!("error: {}", e);
            eprintln!("run `riversd --help` for usage");
            std::process::exit(1);
        }
    };

    // Handle non-server commands first (no logging setup needed)
    match &args.command {
        CliCommand::Version => {
            println!("{}", riversd::cli::version_string());
            return;
        }
        CliCommand::Help => {
            print!("{}", riversd::cli::help_text());
            return;
        }
        CliCommand::Serve => {}
    }

    // Resolve config path: explicit flag > discovery > defaults
    let (config, resolved_config_path) = if let Some(ref path) = args.config_path {
        match rivers_runtime::loader::load_server_config(Path::new(path)) {
            Ok(config) => (config, Some(path.clone())),
            Err(e) => {
                eprintln!("error: failed to load config {}: {}", path.display(), e);
                std::process::exit(1);
            }
        }
    } else if let Some(path) = discover_config() {
        match rivers_runtime::loader::load_server_config(&path) {
            Ok(config) => (config, Some(path)),
            Err(e) => {
                eprintln!("error: failed to load discovered config {}: {}", path.display(), e);
                std::process::exit(1);
            }
        }
    } else {
        (ServerConfig::default(), None)
    };

    // Set up logging: CLI --log-level overrides config
    let log_level = args
        .log_level
        .as_deref()
        .unwrap_or_else(|| match config.base.log_level {
            rivers_runtime::rivers_core::event::LogLevel::Error => "error",
            rivers_runtime::rivers_core::event::LogLevel::Warn => "warn",
            rivers_runtime::rivers_core::event::LogLevel::Info => "info",
            rivers_runtime::rivers_core::event::LogLevel::Debug => "debug",
            rivers_runtime::rivers_core::event::LogLevel::Trace => "trace",
        })
        .to_string();

    let initial_filter = tracing_subscriber::EnvFilter::try_new(&log_level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    // Wrap the filter in a reload layer so the admin API can change it at runtime.
    let (reloadable_filter, filter_handle) =
        tracing_subscriber::reload::Layer::new(initial_filter);
    let filter_handle = std::sync::Arc::new(filter_handle);

    // Optional file appender alongside stdout
    let use_json = config.base.logging.format == "json";

    let _file_guard = if let Some(ref file_path) = config.base.logging.local_file_path {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_path)
            .unwrap_or_else(|e| {
                eprintln!("error: cannot open log file {}: {}", file_path, e);
                std::process::exit(1);
            });
        let (non_blocking, guard) = tracing_appender::non_blocking(file);

        if use_json {
            tracing_subscriber::registry()
                .with(reloadable_filter)
                .with(tracing_subscriber::fmt::layer().json().with_writer(std::io::stdout))
                .with(tracing_subscriber::fmt::layer().json().with_writer(non_blocking))
                .init();
        } else {
            tracing_subscriber::registry()
                .with(reloadable_filter)
                .with(tracing_subscriber::fmt::layer().with_writer(std::io::stdout))
                .with(tracing_subscriber::fmt::layer().with_writer(non_blocking))
                .init();
        }
        Some(guard)
    } else {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        if use_json {
            tracing_subscriber::registry()
                .with(reloadable_filter)
                .with(tracing_subscriber::fmt::layer().json())
                .init();
        } else {
            tracing_subscriber::registry()
                .with(reloadable_filter)
                .with(tracing_subscriber::fmt::layer())
                .init();
        }
        None
    };

    // Build the log controller from the reload handle for the admin API.
    let handle_for_closure = std::sync::Arc::clone(&filter_handle);
    let log_controller = std::sync::Arc::new(riversd::server::LogController::new(
        log_level.clone(),
        move |filter_str| {
            let filter = tracing_subscriber::EnvFilter::try_new(filter_str)
                .map_err(|e| e.to_string())?;
            handle_for_closure.reload(filter).map_err(|e| e.to_string())
        },
    ));

    match resolved_config_path {
        Some(ref path) => tracing::info!(path = %path.display(), "loaded config"),
        None => tracing::debug!("no config file found, using defaults"),
    }
    if config.base.logging.local_file_path.is_some() {
        tracing::info!(
            path = config.base.logging.local_file_path.as_deref().unwrap(),
            "file logging enabled"
        );
    }

    // Wire --no-admin-auth CLI flag into config
    let mut config = config;
    if args.no_admin_auth {
        config.base.admin_api.no_auth = Some(true);
    }

    // Warn if admin auth is disabled
    if config.base.admin_api.no_auth == Some(true) {
        tracing::warn!("--no-admin-auth is active: admin API authentication is DISABLED");
    }

    // Serve
    let (_shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    if args.no_ssl {
        let port = args
            .no_ssl_port
            .or(Some(config.base.port))
            .unwrap();
        tracing::info!(
            host = %config.base.host,
            port = %port,
            "riversd starting"
        );
        if let Err(e) =
            riversd::server::run_server_no_ssl(config, port, shutdown_rx).await
        {
            tracing::error!(error = %e, "server failed");
            std::process::exit(1);
        }
    } else {
        tracing::info!(
            host = %config.base.host,
            port = %config.base.port,
            "riversd starting"
        );

        let addr: std::net::SocketAddr =
            format!("{}:{}", config.base.host, config.base.port)
                .parse()
                .expect("invalid server address");

        let listener = match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!(error = %e, addr = %addr, "failed to bind");
                std::process::exit(1);
            }
        };

        if let Err(e) =
            riversd::server::run_server_with_listener_and_log(
                config,
                listener,
                shutdown_rx,
                Some(log_controller),
            )
            .await
        {
            tracing::error!(error = %e, "server failed");
            std::process::exit(1);
        }
    }
}

/// Discover a config file from conventional locations relative to the binary.
///
/// Probes in order:
/// 1. `./config/riversd.toml`  — same-directory layout (dev / custom installs)
/// 2. `../config/riversd.toml` — release layout (`bin/riversd` next to `config/`)
///
/// Returns the first path that exists as a file.
fn discover_config() -> Option<PathBuf> {
    let candidates = [
        PathBuf::from("config/riversd.toml"),
        PathBuf::from("../config/riversd.toml"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}
