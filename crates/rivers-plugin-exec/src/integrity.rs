//! SHA-256 integrity checking for ExecDriver commands (spec section 6).
//!
//! Three modes control when the driver re-hashes a command binary:
//! - `EachTime`    — hash on every invocation (most secure, highest cost)
//! - `StartupOnly` — hash once at connect(); runtime tampering undetected
//! - `Every(n)`    — hash every N invocations (compromise between the two)

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use rivers_driver_sdk::DriverError;
use sha2::{Digest, Sha256};

use crate::config::IntegrityMode;

// ── File hashing ──────────────────────────────────────────────────────

/// Compute the SHA-256 digest of a file at `path`.
pub fn hash_file(path: &Path) -> Result<[u8; 32], DriverError> {
    let bytes = std::fs::read(path).map_err(|e| {
        DriverError::Internal(format!("cannot read {}: {e}", path.display()))
    })?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.into())
}

// ── Runtime integrity checker ─────────────────────────────────────────

/// Per-command integrity state, created at startup and consulted before
/// each invocation.
pub struct CommandIntegrity {
    mode: IntegrityMode,
    exec_count: AtomicU64,
    pinned_hash: [u8; 32],
}

impl CommandIntegrity {
    /// Create a new integrity checker with the given mode and pinned hash.
    ///
    /// The `pinned_hash` is typically the result of `verify_at_startup`.
    pub fn new(mode: IntegrityMode, pinned_hash: [u8; 32]) -> Self {
        Self {
            mode,
            exec_count: AtomicU64::new(0),
            pinned_hash,
        }
    }

    /// Whether the integrity hash should be checked on this invocation.
    ///
    /// **For `Every(n)` mode**: increments the execution counter and returns
    /// `true` every N calls.  This method must only be called **after**
    /// semaphore acquisition so that rejected concurrency attempts (which
    /// never actually run the command) do not consume scheduled checks.
    /// (RW1.2.e)
    ///
    /// For `EachTime` and `StartupOnly` this is side-effect-free.
    pub fn should_check(&self) -> bool {
        match &self.mode {
            IntegrityMode::EachTime => true,
            IntegrityMode::StartupOnly => false,
            IntegrityMode::Every(n) => {
                // Increment only here — after the caller has acquired the
                // semaphore — so rejected attempts don't burn scheduled checks.
                (self.exec_count.fetch_add(1, Ordering::Relaxed) + 1) % n == 0
            }
        }
    }

    /// Verify the file at `path` still matches the pinned hash.
    ///
    /// Returns `DriverError::Internal` on mismatch.
    pub fn verify(&self, path: &Path) -> Result<(), DriverError> {
        let actual = hash_file(path)?;
        if actual != self.pinned_hash {
            let expected_hex = hex::encode(self.pinned_hash);
            let actual_hex = hex::encode(actual);
            return Err(DriverError::Internal(format!(
                "integrity check failed for command: expected {expected_hex}, got {actual_hex}"
            )));
        }
        Ok(())
    }
}

// ── Startup verification ──────────────────────────────────────────────

/// One-shot startup check: decode the expected hex hash, hash the file,
/// and compare.  On success returns the 32-byte hash so the caller can
/// pass it to `CommandIntegrity::new`.
///
/// Uses `DriverError::Connection` because a mismatch at startup is a
/// configuration / deployment error — the driver cannot connect.
pub fn verify_at_startup(path: &Path, expected_hex: &str) -> Result<[u8; 32], DriverError> {
    let expected_bytes: [u8; 32] = hex::decode(expected_hex)
        .map_err(|e| {
            DriverError::Connection(format!(
                "integrity check failed: invalid hex in sha256 field: {e}"
            ))
        })?
        .try_into()
        .map_err(|v: Vec<u8>| {
            DriverError::Connection(format!(
                "integrity check failed: sha256 must be 32 bytes (64 hex chars), got {} bytes",
                v.len()
            ))
        })?;

    let actual = hash_file(path).map_err(|e| {
        DriverError::Connection(format!("integrity check failed: {e}"))
    })?;

    if actual != expected_bytes {
        let actual_hex = hex::encode(actual);
        return Err(DriverError::Connection(format!(
            "integrity check failed: expected {expected_hex}, got {actual_hex}"
        )));
    }

    Ok(actual)
}

// ── Logging ───────────────────────────────────────────────────────────

/// Log the integrity mode chosen for a command at startup.
pub fn log_integrity_mode(datasource: &str, command: &str, mode: &IntegrityMode) {
    match mode {
        IntegrityMode::EachTime => {
            tracing::info!(datasource, command, integrity_check = "each_time");
        }
        IntegrityMode::StartupOnly => {
            tracing::warn!(
                datasource,
                command,
                integrity_check = "startup_only",
                "script integrity checked at startup only — runtime tampering not detected"
            );
        }
        IntegrityMode::Every(n) => {
            tracing::warn!(
                datasource,
                command,
                integrity_check = %format!("every:{n}"),
                "script integrity checked every {n} executions — tamper detection window applies"
            );
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Create a temp file with known content and return (file, sha256_hex, sha256_bytes).
    fn make_temp_file(content: &[u8]) -> (NamedTempFile, String, [u8; 32]) {
        let mut f = NamedTempFile::new().expect("create temp file");
        f.write_all(content).expect("write temp file");
        f.flush().expect("flush temp file");
        let hash_bytes: [u8; 32] = Sha256::digest(content).into();
        let hash_hex = hex::encode(hash_bytes);
        (f, hash_hex, hash_bytes)
    }

    // ── hash_file ─────────────────────────────────────────────────────

    #[test]
    fn hash_file_known_content() {
        let (file, expected_hex, expected_bytes) = make_temp_file(b"hello world\n");
        let actual = hash_file(file.path()).unwrap();
        assert_eq!(actual, expected_bytes);
        assert_eq!(hex::encode(actual), expected_hex);
    }

    #[test]
    fn hash_file_nonexistent() {
        let result = hash_file(Path::new("/nonexistent/path/to/file"));
        match result {
            Err(DriverError::Internal(msg)) => {
                assert!(msg.contains("cannot read"), "unexpected message: {msg}");
            }
            other => panic!("expected Internal error, got {other:?}"),
        }
    }

    // ── should_check ──────────────────────────────────────────────────

    #[test]
    fn should_check_each_time_always_true() {
        let ci = CommandIntegrity::new(IntegrityMode::EachTime, [0u8; 32]);
        for _ in 0..10 {
            assert!(ci.should_check());
        }
    }

    #[test]
    fn should_check_startup_only_always_false() {
        let ci = CommandIntegrity::new(IntegrityMode::StartupOnly, [0u8; 32]);
        for _ in 0..10 {
            assert!(!ci.should_check());
        }
    }

    #[test]
    fn should_check_every_3() {
        let ci = CommandIntegrity::new(IntegrityMode::Every(3), [0u8; 32]);
        // Calls: 1(f), 2(f), 3(t), 4(f), 5(f), 6(t), 7(f), 8(f), 9(t)
        let results: Vec<bool> = (0..9).map(|_| ci.should_check()).collect();
        assert_eq!(
            results,
            vec![false, false, true, false, false, true, false, false, true]
        );
    }

    // ── verify ────────────────────────────────────────────────────────

    #[test]
    fn verify_matching_hash_passes() {
        let (file, _hex, bytes) = make_temp_file(b"test content");
        let ci = CommandIntegrity::new(IntegrityMode::EachTime, bytes);
        ci.verify(file.path()).unwrap();
    }

    #[test]
    fn verify_mismatched_hash_fails() {
        let (file, _hex, _bytes) = make_temp_file(b"test content");
        let wrong_hash = [0xffu8; 32];
        let ci = CommandIntegrity::new(IntegrityMode::EachTime, wrong_hash);
        match ci.verify(file.path()) {
            Err(DriverError::Internal(msg)) => {
                assert!(
                    msg.contains("integrity check failed for command"),
                    "unexpected message: {msg}"
                );
                assert!(msg.contains("expected"), "should contain expected hash: {msg}");
                assert!(msg.contains("got"), "should contain actual hash: {msg}");
            }
            other => panic!("expected Internal error, got {other:?}"),
        }
    }

    // ── verify_at_startup ─────────────────────────────────────────────

    #[test]
    fn verify_at_startup_valid_passes() {
        let (file, hex_str, expected_bytes) = make_temp_file(b"startup test");
        let result = verify_at_startup(file.path(), &hex_str).unwrap();
        assert_eq!(result, expected_bytes);
    }

    #[test]
    fn verify_at_startup_invalid_hex() {
        let (file, _hex, _bytes) = make_temp_file(b"data");
        match verify_at_startup(file.path(), "not-valid-hex!!") {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("invalid hex"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Connection error, got {other:?}"),
        }
    }

    #[test]
    fn verify_at_startup_wrong_length_hex() {
        let (file, _hex, _bytes) = make_temp_file(b"data");
        // Valid hex but only 4 bytes instead of 32
        match verify_at_startup(file.path(), "aabbccdd") {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("32 bytes"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected Connection error, got {other:?}"),
        }
    }

    #[test]
    fn verify_at_startup_hash_mismatch() {
        let (file, _hex, _bytes) = make_temp_file(b"actual content");
        let wrong_hex = "ff".repeat(32); // 64 hex chars = 32 bytes, all 0xff
        match verify_at_startup(file.path(), &wrong_hex) {
            Err(DriverError::Connection(msg)) => {
                assert!(
                    msg.contains("integrity check failed"),
                    "unexpected message: {msg}"
                );
                assert!(msg.contains("expected"), "should contain expected hash: {msg}");
                assert!(msg.contains("got"), "should contain actual hash: {msg}");
            }
            other => panic!("expected Connection error, got {other:?}"),
        }
    }
}
