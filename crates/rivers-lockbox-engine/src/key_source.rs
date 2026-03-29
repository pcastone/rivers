//! Key source resolution -- reading the Age identity from env, file, or agent.

use std::path::Path;

use crate::types::{LockBoxConfig, LockBoxError};

// ── Key Source Resolution ───────────────────────────────────────────

/// Read the Age identity string from the configured key source.
///
/// Per spec S6.
pub fn resolve_key_source(config: &LockBoxConfig) -> Result<String, LockBoxError> {
    match config.key_source.as_str() {
        "env" => {
            let var_name = &config.key_env_var;
            let value = std::env::var(var_name).map_err(|_| LockBoxError::KeySourceUnavailable {
                reason: format!("environment variable {} is not set", var_name),
            })?;

            // NOTE: Previously called `std::env::remove_var` here, but that is
            // UB in Rust 1.77+ when other threads may read env vars. The Tokio
            // runtime is already running at this point so we cannot guarantee
            // single-threaded access. The env var remains set; callers should
            // avoid persisting secrets in environment variables in production.

            if value.is_empty() {
                return Err(LockBoxError::KeySourceUnavailable {
                    reason: format!("environment variable {} is empty", var_name),
                });
            }

            Ok(value)
        }

        "file" => {
            let key_file = config.key_file.as_deref().ok_or_else(|| {
                LockBoxError::KeySourceUnavailable {
                    reason: "key_source = \"file\" but key_file is not set".to_string(),
                }
            })?;

            // Check file permissions
            check_file_permissions(Path::new(key_file))?;

            std::fs::read_to_string(key_file).map_err(|e| LockBoxError::KeySourceUnavailable {
                reason: format!("cannot read key file {}: {}", key_file, e),
            })
        }

        "agent" => {
            // Agent support is stubbed -- requires SSH agent integration
            Err(LockBoxError::KeySourceUnavailable {
                reason: "key_source = \"agent\" is not yet supported".to_string(),
            })
        }

        other => Err(LockBoxError::KeySourceUnavailable {
            reason: format!("unknown key_source: \"{}\" -- must be env, file, or agent", other),
        }),
    }
}

// ── File Permission Checks ──────────────────────────────────────────

/// Check that a file has mode 600 (owner read+write only).
///
/// Per spec S6 -- enforced on both .rkeystore and key_file.
#[cfg(unix)]
pub fn check_file_permissions(path: &Path) -> Result<(), LockBoxError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            LockBoxError::KeystoreNotFound {
                path: path.display().to_string(),
            }
        } else {
            LockBoxError::Io(e)
        }
    })?;

    let mode = metadata.mode() & 0o777;
    if mode != 0o600 {
        return Err(LockBoxError::InsecureFilePermissions {
            path: path.display().to_string(),
            mode,
        });
    }

    Ok(())
}

#[cfg(not(unix))]
pub fn check_file_permissions(_path: &Path) -> Result<(), LockBoxError> {
    // File permission checks only apply on Unix
    Ok(())
}
