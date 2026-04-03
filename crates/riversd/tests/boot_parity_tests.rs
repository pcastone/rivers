//! Boot path parity tests — verify --no-ssl path has same subsystems as TLS path.
//!
//! BUG-005: The --no-ssl code path was missing StorageEngine, SessionManager,
//! CsrfManager, EventBus logging, engine loader, and host context wiring.
//! These tests verify both paths produce equivalent AppContext state.

use rivers_runtime::rivers_core::config::ServerConfig;

/// BUG-005 regression: verify the --no-ssl lifecycle source code contains
/// all the subsystem initialization calls that the TLS path has.
///
/// This is a source-level parity test — it reads the lifecycle.rs file
/// and checks for required initialization patterns in both code paths.
#[test]
fn no_ssl_path_has_all_subsystem_init_calls() {
    let lifecycle_src = include_str!("../src/server/lifecycle.rs");

    // Find the no-ssl function body
    let no_ssl_start = lifecycle_src
        .find("pub async fn run_server_no_ssl")
        .expect("run_server_no_ssl function not found");
    let no_ssl_end = lifecycle_src[no_ssl_start..]
        .find("\npub ")
        .map(|i| no_ssl_start + i)
        .unwrap_or(lifecycle_src.len());
    let no_ssl_body = &lifecycle_src[no_ssl_start..no_ssl_end];

    // Required subsystem initialization patterns that MUST exist in the --no-ssl path
    let required_patterns = [
        ("initialize_runtime", "runtime initialization (DataView engine, gossip)"),
        ("create_storage_engine", "StorageEngine creation"),
        ("SessionManager::new", "SessionManager initialization"),
        ("CsrfManager::new", "CsrfManager initialization"),
        ("log_handler", "EventBus LogHandler registration"),
        ("load_and_wire_bundle", "bundle loading"),
        ("set_host_context", "engine host context wiring"),
    ];

    let mut missing = Vec::new();
    for (pattern, description) in &required_patterns {
        if !no_ssl_body.contains(pattern) {
            missing.push(format!("  - {} ('{}')", description, pattern));
        }
    }

    assert!(
        missing.is_empty(),
        "BUG-005 regression: --no-ssl path is missing subsystem initialization:\n{}",
        missing.join("\n")
    );
}

/// Verify the TLS path also has all patterns (sanity check for the test itself).
#[test]
fn tls_path_has_all_subsystem_init_calls() {
    let lifecycle_src = include_str!("../src/server/lifecycle.rs");

    let tls_start = lifecycle_src
        .find("pub async fn run_server_with_listener_and_log")
        .expect("run_server_with_listener_and_log function not found");
    let tls_end = lifecycle_src[tls_start..]
        .find("\n// ── ")
        .map(|i| tls_start + i)
        .unwrap_or(lifecycle_src.len());
    let tls_body = &lifecycle_src[tls_start..tls_end];

    let required_patterns = [
        "initialize_runtime",
        "create_storage_engine",
        "SessionManager::new",
        "CsrfManager::new",
        "load_and_wire_bundle",
        "set_host_context",
    ];

    for pattern in &required_patterns {
        assert!(
            tls_body.contains(pattern),
            "TLS path missing '{}' — test sanity check failed (is the code refactored?)",
            pattern
        );
    }
}

/// BUG-013 regression: module paths must be resolved to absolute during bundle load.
/// Verify the resolve_handler_module function exists in load.rs.
#[test]
fn module_path_resolution_exists_in_bundle_loader() {
    let load_src = include_str!("../src/bundle_loader/load.rs");

    assert!(
        load_src.contains("resolve_handler_module"),
        "BUG-013: module path resolution function missing from bundle loader"
    );
    assert!(
        load_src.contains("app_dir.join"),
        "BUG-013: module paths not being joined with app_dir"
    );
}

/// Verify the config struct has the storage_engine field with a default.
#[test]
fn storage_engine_config_has_memory_default() {
    let config = ServerConfig::default();
    assert_eq!(
        config.storage_engine.backend, "memory",
        "StorageEngine default backend should be 'memory'"
    );
}
