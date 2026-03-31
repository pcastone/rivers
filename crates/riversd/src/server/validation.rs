//! Validation helpers, error types, shutdown signal, and hot reload watcher.

use std::sync::Arc;

use tokio::sync::watch;

use rivers_runtime::rivers_core::ServerConfig;

use crate::hot_reload::{FileWatcher, HotReloadState};

// ── Validation ────────────────────────────────────────────────────

/// Validate admin server access control rules.
///
/// SHAPE-25: TLS is mandatory (validated separately via validate_admin_tls_config).
/// Ed25519 public_key is required regardless of bind address.
/// The localhost plain-HTTP exception is removed.
pub fn validate_admin_access_control(
    admin: &rivers_runtime::rivers_core::config::AdminApiConfig,
) -> Result<(), String> {
    if admin.public_key.is_none() {
        return Err(
            "admin API requires public_key to be configured (Ed25519 auth is mandatory)".to_string()
        );
    }
    Ok(())
}

/// Validate TLS configuration at startup.
///
/// Per spec §2: `[base.tls]` is required unless `--no-ssl` is active.
/// When `no_ssl = true`, skips all TLS validation.
/// SHAPE-25: admin TLS is always validated when admin_api is enabled (no --no-ssl bypass).
pub fn validate_server_tls(config: &ServerConfig, no_ssl: bool) -> Result<(), String> {
    // SHAPE-25: admin TLS is always required regardless of --no-ssl
    if config.base.admin_api.enabled {
        crate::tls::validate_admin_tls_config(&config.base.admin_api.tls)?;
    }

    if no_ssl {
        return Ok(());
    }

    // Main server TLS checks (skipped when --no-ssl)
    crate::tls::validate_tls_config(&config.base.tls)?;

    if let Some(ref tls) = config.base.tls {
        crate::tls::validate_redirect_port(config.base.port, tls.redirect_port)?;
    }

    if config.base.http2.enabled && config.base.tls.is_none() {
        return Err("HTTP/2 requires TLS: add [base.tls] to your config".to_string());
    }

    Ok(())
}

// ── Error Types ───────────────────────────────────────────────────

/// Server startup/runtime errors.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    /// Configuration validation failure.
    #[error("config error: {0}")]
    Config(String),

    /// Failed to bind to the configured address/port.
    #[error("bind error: {0}")]
    Bind(String),

    /// Error while serving requests.
    #[error("serve error: {0}")]
    Serve(String),
}

// ── Shutdown Signal ───────────────────────────────────────────────

/// Wait for a shutdown signal.
///
/// Per spec §13.1: SIGTERM, SIGINT, or watch channel.
pub async fn shutdown_signal(mut rx: watch::Receiver<bool>) {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    let sigterm = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let sigterm = std::future::pending::<()>();

    let watch = async move {
        while rx.changed().await.is_ok() {
            if *rx.borrow() {
                return;
            }
        }
    };

    tokio::select! {
        _ = ctrl_c => {}
        _ = sigterm => {}
        _ = watch => {}
    }
}

// ── Hot Reload Watcher ────────────────────────────────────────────

/// Spawn a hot-reload file watcher if a config path is available.
///
/// Per spec §2 step 21 / §16: dev mode only, non-fatal on failure.
pub(super) fn maybe_spawn_hot_reload_watcher(
    config_path: Option<&std::path::Path>,
    state: Arc<HotReloadState>,
) -> Option<FileWatcher> {
    let path = config_path?;
    match FileWatcher::new(path.to_path_buf(), state) {
        Ok(watcher) => Some(watcher),
        Err(e) => {
            tracing::warn!(error = %e, "hot reload watcher failed to start — continuing without");
            None
        }
    }
}
