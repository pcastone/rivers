//! Config file discovery for Rivers binaries.
//!
//! Shared by `riversd` and `riversctl` so both find `riversd.toml` the same way.
//!
//! Search order:
//! 1. `$RIVERS_HOME/config/riversd.toml`
//! 2. Binary's `../config/riversd.toml` (standard install layout)
//! 3. `./config/riversd.toml` (CWD)
//! 4. `/etc/rivers/riversd.toml` (system-wide)
//!
//! All other paths (apphome, lib, plugins, logs, lockbox) are configured
//! inside `riversd.toml` and can be absolute or relative to CWD.

use std::path::PathBuf;

/// Discover `riversd.toml` from conventional locations.
///
/// Returns the first path that exists as a file.
pub fn discover_config() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    // 1. $RIVERS_HOME/config
    if let Ok(home) = std::env::var("RIVERS_HOME") {
        candidates.push(PathBuf::from(home).join("config/riversd.toml"));
    }

    // 2. Binary's ../config (e.g. /opt/rivers/bin/riversd → /opt/rivers/config/)
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(exe) = exe.canonicalize() {
            if let Some(bin_dir) = exe.parent() {
                if let Some(root) = bin_dir.parent() {
                    candidates.push(root.join("config/riversd.toml"));
                }
            }
        }
    }

    // 3. CWD-relative
    candidates.push(PathBuf::from("config/riversd.toml"));

    // 4. System-wide
    candidates.push(PathBuf::from("/etc/rivers/riversd.toml"));

    // Deduplicate (e.g. if CWD is the install root, #2 and #3 overlap)
    candidates.dedup();

    candidates.into_iter().find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_config_does_not_panic() {
        // Just verify it runs without panicking in any environment
        let _ = discover_config();
    }
}
