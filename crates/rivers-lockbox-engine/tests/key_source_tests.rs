//! Key source resolution tests -- env, file, agent, unknown sources.

use age::secrecy::ExposeSecret;
use rivers_lockbox_engine::*;

/// Helper: generate an Age keypair, returning (identity_string, recipient_string).
fn generate_keypair() -> (String, String) {
    let identity = age::x25519::Identity::generate();
    let identity_str = identity.to_string().expose_secret().to_string();
    let recipient_str = identity.to_public().to_string();
    (identity_str, recipient_str)
}

// ── Key Source: env ──────────────────────────────────────────────

#[test]
fn resolve_key_source_env_success() {
    let (identity_str, _) = generate_keypair();
    let env_var = "TEST_LOCKBOX_KEY_ENV_1";
    // SAFETY: set_var is not thread-safe, but each test uses a unique var name.
    unsafe { std::env::set_var(env_var, &identity_str); }

    let config = LockBoxConfig {
        key_source: "env".to_string(),
        key_env_var: env_var.to_string(),
        ..Default::default()
    };

    let result = resolve_key_source(&config).unwrap();
    assert_eq!(result, identity_str);
}

#[test]
fn resolve_key_source_env_missing_var() {
    let config = LockBoxConfig {
        key_source: "env".to_string(),
        key_env_var: "TEST_LOCKBOX_KEY_ENV_MISSING_9999".to_string(),
        ..Default::default()
    };

    let result = resolve_key_source(&config);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::KeySourceUnavailable { reason } => {
            assert!(reason.contains("not set"), "reason was: {}", reason);
        }
        other => panic!("expected KeySourceUnavailable, got: {:?}", other),
    }
}

#[test]
fn resolve_key_source_env_empty_var() {
    let env_var = "TEST_LOCKBOX_KEY_ENV_EMPTY_2";
    unsafe { std::env::set_var(env_var, ""); }

    let config = LockBoxConfig {
        key_source: "env".to_string(),
        key_env_var: env_var.to_string(),
        ..Default::default()
    };

    let result = resolve_key_source(&config);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::KeySourceUnavailable { reason } => {
            assert!(reason.contains("empty"), "reason was: {}", reason);
        }
        other => panic!("expected KeySourceUnavailable, got: {:?}", other),
    }
}

// ── Key Source: file ─────────────────────────────────────────────

#[cfg(unix)]
#[test]
fn resolve_key_source_file_success() {
    use std::os::unix::fs::PermissionsExt;

    let (identity_str, _) = generate_keypair();
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("age.key");
    std::fs::write(&key_file, &identity_str).unwrap();
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o600)).unwrap();

    let config = LockBoxConfig {
        key_source: "file".to_string(),
        key_file: Some(key_file.to_str().unwrap().to_string()),
        ..Default::default()
    };

    let result = resolve_key_source(&config).unwrap();
    assert_eq!(result, identity_str);
}

#[test]
fn resolve_key_source_file_missing_path() {
    let config = LockBoxConfig {
        key_source: "file".to_string(),
        key_file: Some("/nonexistent/age.key".to_string()),
        ..Default::default()
    };

    let result = resolve_key_source(&config);
    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn resolve_key_source_file_insecure_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("age.key");
    std::fs::write(&key_file, "AGE-SECRET-KEY-DUMMY").unwrap();
    std::fs::set_permissions(&key_file, std::fs::Permissions::from_mode(0o644)).unwrap();

    let config = LockBoxConfig {
        key_source: "file".to_string(),
        key_file: Some(key_file.to_str().unwrap().to_string()),
        ..Default::default()
    };

    let result = resolve_key_source(&config);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::InsecureFilePermissions { mode, .. } => {
            assert_eq!(mode, 0o644);
        }
        other => panic!("expected InsecureFilePermissions, got: {:?}", other),
    }
}

// ── Key Source: agent ────────────────────────────────────────────

#[test]
fn resolve_key_source_agent_unsupported() {
    let config = LockBoxConfig {
        key_source: "agent".to_string(),
        agent_socket: Some("/tmp/age-agent.sock".to_string()),
        ..Default::default()
    };

    let result = resolve_key_source(&config);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::KeySourceUnavailable { reason } => {
            assert!(reason.contains("not yet supported"), "reason was: {}", reason);
        }
        other => panic!("expected KeySourceUnavailable, got: {:?}", other),
    }
}

// ── Key Source: unknown ──────────────────────────────────────────

#[test]
fn resolve_key_source_unknown_source() {
    let config = LockBoxConfig {
        key_source: "magic".to_string(),
        ..Default::default()
    };

    let result = resolve_key_source(&config);
    assert!(result.is_err());
    match result.unwrap_err() {
        LockBoxError::KeySourceUnavailable { reason } => {
            assert!(reason.contains("magic"), "reason was: {}", reason);
        }
        other => panic!("expected KeySourceUnavailable, got: {:?}", other),
    }
}
