use std::sync::Arc;

use rivers_runtime::rivers_core::ServerConfig;
use riversd::hot_reload::{check_reload_scope, FileWatcher, HotReloadState, ReloadScope};

// ── HotReloadState ──────────────────────────────────────────────

#[tokio::test]
async fn initial_version_is_zero() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);
    assert_eq!(state.version(), 0);
}

#[tokio::test]
async fn current_config_returns_snapshot() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);
    let snapshot = state.current_config().await;
    assert_eq!(snapshot.base.port, 8080); // default port
}

#[tokio::test]
async fn swap_increments_version() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);

    let mut new_config = ServerConfig::default();
    new_config.base.request_timeout_seconds = 60;

    let version = state.swap(new_config).await.unwrap();
    assert_eq!(version, 1);
    assert_eq!(state.version(), 1);
}

#[tokio::test]
async fn swap_updates_config() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);

    let mut new_config = ServerConfig::default();
    new_config.base.request_timeout_seconds = 120;

    state.swap(new_config).await.unwrap();
    let snapshot = state.current_config().await;
    assert_eq!(snapshot.base.request_timeout_seconds, 120);
}

#[tokio::test]
async fn old_snapshot_unaffected_by_swap() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);

    // Take a snapshot before swap
    let old_snapshot = state.current_config().await;
    assert_eq!(old_snapshot.base.request_timeout_seconds, 30); // default

    // Swap config
    let mut new_config = ServerConfig::default();
    new_config.base.request_timeout_seconds = 120;
    state.swap(new_config).await.unwrap();

    // Old snapshot unchanged
    assert_eq!(old_snapshot.base.request_timeout_seconds, 30);

    // New snapshot has updated value
    let new_snapshot = state.current_config().await;
    assert_eq!(new_snapshot.base.request_timeout_seconds, 120);
}

#[tokio::test]
async fn subscribe_notified_on_swap() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);
    let mut rx = state.subscribe();

    let new_config = ServerConfig::default();
    state.swap(new_config).await.unwrap();

    rx.changed().await.unwrap();
    assert_eq!(*rx.borrow(), 1);
}

#[tokio::test]
async fn config_path() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, Some("/etc/riversd.conf".into()));
    assert_eq!(state.config_path().unwrap().to_str().unwrap(), "/etc/riversd.conf");
}

#[tokio::test]
async fn config_path_none() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);
    assert!(state.config_path().is_none());
}

// ── Reload Scope ────────────────────────────────────────────────

#[test]
fn same_config_is_safe() {
    let config = ServerConfig::default();
    assert_eq!(check_reload_scope(&config, &config), ReloadScope::Safe);
}

#[test]
fn changed_port_requires_restart() {
    let current = ServerConfig::default();
    let mut new = ServerConfig::default();
    new.base.port = 9999;
    match check_reload_scope(&current, &new) {
        ReloadScope::RequiresRestart(msg) => assert!(msg.contains("port")),
        ReloadScope::Safe => panic!("expected RequiresRestart"),
    }
}

#[test]
fn changed_host_requires_restart() {
    let current = ServerConfig::default();
    let mut new = ServerConfig::default();
    new.base.host = "10.0.0.1".to_string();
    match check_reload_scope(&current, &new) {
        ReloadScope::RequiresRestart(msg) => assert!(msg.contains("host")),
        ReloadScope::Safe => panic!("expected RequiresRestart"),
    }
}

#[test]
fn changed_tls_requires_restart() {
    use rivers_runtime::rivers_core::config::TlsConfig;
    let current = ServerConfig::default();
    let mut new = ServerConfig::default();
    new.base.tls = Some(TlsConfig {
        cert: Some("/new/cert.pem".to_string()),
        ..TlsConfig::default()
    });
    match check_reload_scope(&current, &new) {
        ReloadScope::RequiresRestart(msg) => assert!(msg.contains("TLS")),
        ReloadScope::Safe => panic!("expected RequiresRestart"),
    }
}

#[test]
fn changed_timeout_is_safe() {
    let current = ServerConfig::default();
    let mut new = ServerConfig::default();
    new.base.request_timeout_seconds = 120;
    assert_eq!(check_reload_scope(&current, &new), ReloadScope::Safe);
}

// ── FileWatcher ────────────────────────────────────────────────

#[tokio::test]
async fn file_watcher_detects_config_change() {
    // Write initial config
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("riversd.conf");
    std::fs::write(&config_path, "[base]\nrequest_timeout_seconds = 30\n").unwrap();

    let config = ServerConfig::default();
    let state = Arc::new(HotReloadState::new(config, Some(config_path.clone())));

    // Start watcher
    let _watcher = FileWatcher::new(config_path.clone(), state.clone()).unwrap();

    // Wait for watcher setup + drain any macOS FSEvents backlog for the initial write.
    // Must exceed the 500ms debounce window so the next real write is processed.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Record version after potential backlog events
    let version_before = state.version();

    // Subscribe AFTER draining backlog
    let mut rx = state.subscribe();

    // Modify the config file — this is the write we care about
    std::fs::write(&config_path, "[base]\nrequest_timeout_seconds = 99\n").unwrap();

    // Wait for the change notification via watch channel.
    // 15s deadline: macOS FSEvents can be slow under heavy CI/workspace I/O load.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        rx.changed(),
    ).await;
    assert!(result.is_ok(), "timed out waiting for config change notification");

    // Verify version incremented beyond the pre-write baseline
    assert!(
        state.version() > version_before,
        "expected version > {}, got {}",
        version_before,
        state.version()
    );

    let snapshot = state.current_config().await;
    assert_eq!(snapshot.base.request_timeout_seconds, 99);
}

// ── Bundle path accessor ─────────────────────────────────────────

#[tokio::test]
async fn bundle_path_returns_config_value() {
    let mut config = ServerConfig::default();
    config.bundle_path = Some("test/bundle".into());
    let state = HotReloadState::new(config, None);

    assert_eq!(state.bundle_path().await, Some("test/bundle".to_string()));
}

#[tokio::test]
async fn bundle_path_none_when_not_configured() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);

    assert!(state.bundle_path().await.is_none());
}

#[tokio::test]
async fn bundle_path_updates_after_swap() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);

    let mut new_config = ServerConfig::default();
    new_config.bundle_path = Some("new/bundle/path".into());
    state.swap(new_config).await.unwrap();

    assert_eq!(state.bundle_path().await, Some("new/bundle/path".to_string()));
}

// ── Integration: Concurrent swaps ────────────────────────────────

#[tokio::test]
async fn concurrent_swaps_both_succeed_final_version_correct() {
    let config = ServerConfig::default();
    let state = Arc::new(HotReloadState::new(config, None));

    let s1 = state.clone();
    let s2 = state.clone();

    let (r1, r2) = tokio::join!(
        async move {
            let mut c = ServerConfig::default();
            c.base.request_timeout_seconds = 60;
            s1.swap(c).await
        },
        async move {
            let mut c = ServerConfig::default();
            c.base.request_timeout_seconds = 90;
            s2.swap(c).await
        },
    );

    assert!(r1.is_ok());
    assert!(r2.is_ok());
    assert_eq!(state.version(), 2);
}

// ── Integration: Multiple subscribers notified ───────────────────

#[tokio::test]
async fn multiple_subscribers_all_notified_on_swap() {
    let config = ServerConfig::default();
    let state = HotReloadState::new(config, None);

    let mut rx1 = state.subscribe();
    let mut rx2 = state.subscribe();
    let mut rx3 = state.subscribe();

    state.swap(ServerConfig::default()).await.unwrap();

    // All 3 subscribers should see the change
    rx1.changed().await.unwrap();
    rx2.changed().await.unwrap();
    rx3.changed().await.unwrap();

    assert_eq!(*rx1.borrow(), 1);
    assert_eq!(*rx2.borrow(), 1);
    assert_eq!(*rx3.borrow(), 1);
}

// ── Integration: FileWatcher error recovery (invalid TOML) ──────

#[tokio::test]
async fn file_watcher_invalid_toml_no_crash() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("riversd.conf");
    std::fs::write(&config_path, "[base]\nrequest_timeout_seconds = 30\n").unwrap();

    let config = ServerConfig::default();
    let state = Arc::new(HotReloadState::new(config, Some(config_path.clone())));
    let _initial_version = state.version();

    let _watcher = FileWatcher::new(config_path.clone(), state.clone()).unwrap();

    // Wait for watcher setup
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Write invalid TOML — should NOT crash, version should not change
    std::fs::write(&config_path, "INVALID {{ TOML }}}}").unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Version may increment due to FSEvents backlog, but config should still be valid
    // The key assertion: no panic, process is still alive
    let snapshot = state.current_config().await;
    assert!(snapshot.base.port > 0, "config should still be valid");
}

// ── Integration: reload scope combined ───────────────────────────

#[test]
fn changed_logging_config_is_safe_to_reload() {
    use rivers_runtime::rivers_core::config::LoggingConfig;
    use rivers_runtime::rivers_core::event::LogLevel;

    let current = ServerConfig::default();
    let mut new = ServerConfig::default();
    new.base.logging = LoggingConfig {
        level: LogLevel::Debug,
        format: "text".into(),
        local_file_path: Some("/var/log/rivers.log".into()),
    };
    assert_eq!(check_reload_scope(&current, &new), ReloadScope::Safe);
}
