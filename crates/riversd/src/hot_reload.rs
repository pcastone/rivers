//! Hot reload (dev mode) support.
//!
//! Per `rivers-httpd-spec.md` §16.
//!
//! Config file watcher that swaps view routes, DataViews, and security config
//! without server restart. In-flight requests use their Arc<ServerConfig> snapshot.
//!
//! Uses mtime polling instead of OS-level file watchers (no `notify` dependency).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use tokio::sync::{watch, RwLock};

use rivers_runtime::rivers_core::ServerConfig;

// ── HotReloadState ──────────────────────────────────────────────

/// Shared config state with RwLock for atomic swap.
///
/// Per spec §16: in-flight requests use their Arc snapshot unaffected by swap.
pub struct HotReloadState {
    /// Current config behind RwLock for atomic swap.
    config: RwLock<Arc<ServerConfig>>,
    /// Watch channel to notify subscribers of config changes.
    change_tx: watch::Sender<u64>,
    /// Monotonic version counter.
    version: std::sync::atomic::AtomicU64,
    /// Config file path being watched.
    config_path: Option<PathBuf>,
}

impl HotReloadState {
    /// Create a new hot reload state with initial config.
    pub fn new(config: ServerConfig, config_path: Option<PathBuf>) -> Self {
        let (change_tx, _) = watch::channel(0);
        Self {
            config: RwLock::new(Arc::new(config)),
            change_tx,
            version: std::sync::atomic::AtomicU64::new(0),
            config_path,
        }
    }

    /// Get the current config snapshot.
    ///
    /// Returns an Arc that won't be affected by subsequent swaps.
    pub async fn current_config(&self) -> Arc<ServerConfig> {
        self.config.read().await.clone()
    }

    /// Swap to a new config atomically.
    ///
    /// Per spec §16.3: acquire lock → validate → swap → notify → release.
    pub async fn swap(&self, new_config: ServerConfig) -> Result<u64, HotReloadError> {
        let version = self
            .version
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;

        let mut config = self.config.write().await;
        *config = Arc::new(new_config);

        // Notify subscribers
        let _ = self.change_tx.send(version);

        tracing::info!(version, "config reloaded");
        Ok(version)
    }

    /// Subscribe to config change notifications.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.change_tx.subscribe()
    }

    /// Current config version.
    pub fn version(&self) -> u64 {
        self.version
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Config file path being watched (if any).
    pub fn config_path(&self) -> Option<&PathBuf> {
        self.config_path.as_ref()
    }

    /// Bundle path from the current config (if set).
    ///
    /// Used by the hot reload listener to re-parse the bundle on config change.
    pub async fn bundle_path(&self) -> Option<String> {
        let config = self.config.read().await;
        config.bundle_path.clone()
    }
}

// ── File Watcher (polling) ─────────────────────────────────────

/// Poll interval for checking config file changes.
const POLL_INTERVAL_SECS: u64 = 2;

/// Watches a config file for changes via mtime polling and triggers hot reload.
///
/// Per spec §16: file watcher with debounce. On change, reads the config,
/// validates scope, and swaps if safe.
pub struct FileWatcher {
    /// Cancel signal — dropped when FileWatcher is dropped, stopping the poll task.
    _cancel: tokio::sync::oneshot::Sender<()>,
}

impl FileWatcher {
    /// Start watching `config_path` for modifications.
    ///
    /// Polls file mtime every 2 seconds. On change:
    /// 1. Read and parse the config file.
    /// 2. Check reload scope against the current config.
    /// 3. If safe, swap the config in `reload_state`.
    /// Errors are logged, not propagated.
    pub fn new(
        config_path: PathBuf,
        reload_state: Arc<HotReloadState>,
    ) -> Result<Self, HotReloadError> {
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();

        let initial_mtime = file_mtime(&config_path).unwrap_or(SystemTime::UNIX_EPOCH);

        let watch_path = config_path.clone();
        tokio::spawn(async move {
            let mut last_mtime = initial_mtime;
            let interval = tokio::time::Duration::from_secs(POLL_INTERVAL_SECS);

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    _ = &mut cancel_rx => {
                        tracing::debug!("file watcher stopped");
                        return;
                    }
                }

                let mtime = match file_mtime(&watch_path) {
                    Some(t) => t,
                    None => continue,
                };

                if mtime > last_mtime {
                    last_mtime = mtime;
                    Self::handle_change(watch_path.clone(), reload_state.clone()).await;
                }
            }
        });

        tracing::info!(path = %config_path.display(), poll_secs = POLL_INTERVAL_SECS, "file watcher started (polling)");

        Ok(Self { _cancel: cancel_tx })
    }

    /// Handle a config file change event.
    async fn handle_change(config_path: PathBuf, reload_state: Arc<HotReloadState>) {
        // Read the config file
        let content = match tokio::fs::read_to_string(&config_path).await {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, path = %config_path.display(), "failed to read config file");
                return;
            }
        };

        // Parse the config
        let new_config: ServerConfig = match toml::from_str(&content) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "failed to parse config file");
                return;
            }
        };

        // Check reload scope
        let current = reload_state.current_config().await;
        match check_reload_scope(&current, &new_config) {
            ReloadScope::Safe => {
                if let Err(e) = reload_state.swap(new_config).await {
                    tracing::error!(error = %e, "config swap failed");
                }
            }
            ReloadScope::RequiresRestart(reason) => {
                tracing::warn!(reason, "config change requires restart, skipping hot reload");
            }
        }
    }
}

/// Get file modification time, or None if the file doesn't exist.
fn file_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

// ── Reload Validation ───────────────────────────────────────────

/// Fields that can be hot-reloaded without restart.
///
/// Per spec §16: views, DataViews, security config are swappable.
/// Bind address, port, and TLS config require restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadScope {
    /// Safe to reload without restart.
    Safe,
    /// Requires restart — bind address or port changed.
    RequiresRestart(String),
}

/// Check if a config change can be hot-reloaded.
pub fn check_reload_scope(current: &ServerConfig, new: &ServerConfig) -> ReloadScope {
    if current.base.host != new.base.host {
        return ReloadScope::RequiresRestart("base.host changed".to_string());
    }
    if current.base.port != new.base.port {
        return ReloadScope::RequiresRestart("base.port changed".to_string());
    }
    let current_cert = current.base.tls.as_ref().and_then(|t| t.cert.as_ref());
    let new_cert = new.base.tls.as_ref().and_then(|t| t.cert.as_ref());
    let current_key = current.base.tls.as_ref().and_then(|t| t.key.as_ref());
    let new_key = new.base.tls.as_ref().and_then(|t| t.key.as_ref());
    if current_cert != new_cert || current_key != new_key {
        return ReloadScope::RequiresRestart("TLS config changed".to_string());
    }

    ReloadScope::Safe
}

// ── Error Types ─────────────────────────────────────────────────

/// Hot reload errors.
#[derive(Debug, thiserror::Error)]
pub enum HotReloadError {
    /// Config file could not be parsed.
    #[error("config parse error: {0}")]
    ParseError(String),

    /// Config validation rejected the new config.
    #[error("config validation failed: {0}")]
    ValidationFailed(String),

    /// File watcher setup or runtime error.
    #[error("file watch error: {0}")]
    WatchError(String),

    /// The config change requires a full server restart.
    #[error("requires restart: {0}")]
    RequiresRestart(String),
}
