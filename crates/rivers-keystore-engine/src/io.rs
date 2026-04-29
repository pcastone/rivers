//! File I/O — create, load, save keystore files with Age encryption.

use std::path::Path;

use zeroize::Zeroize;

use crate::types::*;

// ── File operations ─────────────────────────────────────────────────

/// Acquire an exclusive advisory lock on a file descriptor.
///
/// Uses `libc::flock(LOCK_EX)` on Unix to serialize concurrent
/// read-modify-write cycles on the keystore file. The lock is released
/// automatically when the file descriptor is closed (i.e., when the
/// `std::fs::File` guard is dropped).
///
/// On non-Unix targets this is a no-op.
#[cfg(unix)]
fn flock_exclusive(file: &std::fs::File) -> Result<(), AppKeystoreError> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    // SAFETY: fd is valid for the lifetime of the File, and LOCK_EX | LOCK_NB
    // are standard flock flags. We deliberately use LOCK_EX (blocking) so that
    // concurrent callers queue rather than fail.
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX) };
    if ret != 0 {
        return Err(AppKeystoreError::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn flock_exclusive(_file: &std::fs::File) -> Result<(), AppKeystoreError> {
    Ok(())
}

impl AppKeystore {
    /// Create a new empty keystore file at `path`, encrypted with the
    /// given Age recipient public key string.
    pub fn create(path: &Path, recipient_key: &str) -> Result<(), AppKeystoreError> {
        let keystore = AppKeystore {
            version: 1,
            keys: Vec::new(),
        };
        keystore.save(path, recipient_key)
    }

    /// Load and decrypt a keystore file from `path` using the given
    /// Age identity (private key) string.
    ///
    /// Acquires an exclusive advisory lock before reading so that a
    /// concurrent `save()` cannot produce a torn read.
    pub fn load(path: &Path, identity_str: &str) -> Result<AppKeystore, AppKeystoreError> {
        // Open the file and acquire an exclusive lock before reading.
        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    AppKeystoreError::KeystoreNotFound {
                        path: path.display().to_string(),
                    }
                } else {
                    AppKeystoreError::Io(e)
                }
            })?;
        flock_exclusive(&lock_file)?;

        // Read encrypted bytes while holding the lock.
        let encrypted = {
            use std::io::Read;
            let mut buf = Vec::new();
            std::io::BufReader::new(&lock_file).read_to_end(&mut buf)?;
            buf
        };
        // Lock is released when lock_file drops at end of this scope.
        drop(lock_file);

        // Parse Age identity
        let identity = identity_str
            .parse::<age::x25519::Identity>()
            .map_err(|_| AppKeystoreError::DecryptionFailed)?;

        // Decrypt
        let mut decrypted =
            age::decrypt(&identity, &encrypted).map_err(|_| AppKeystoreError::DecryptionFailed)?;

        // Parse TOML
        let toml_str =
            std::str::from_utf8(&decrypted).map_err(|_| AppKeystoreError::MalformedKeystore {
                reason: "decrypted payload is not valid UTF-8".to_string(),
            })?;

        let keystore: AppKeystore =
            toml::from_str(toml_str).map_err(|e| AppKeystoreError::MalformedKeystore {
                reason: e.to_string(),
            })?;

        // Zeroize decrypted bytes
        decrypted.zeroize();

        Ok(keystore)
    }

    /// Serialize and encrypt this keystore, writing it to `path`.
    ///
    /// Uses an exclusive advisory lock, a tempfile, fsync, and atomic rename
    /// to guard against concurrent saves and OS crashes leaving a torn file.
    pub fn save(&self, path: &Path, recipient_key: &str) -> Result<(), AppKeystoreError> {
        // Serialize to TOML
        let mut toml_str =
            toml::to_string_pretty(self).map_err(|e| AppKeystoreError::MalformedKeystore {
                reason: format!("serialization failed: {}", e),
            })?;

        // Parse recipient
        let recipient: age::x25519::Recipient =
            recipient_key
                .parse()
                .map_err(|_| AppKeystoreError::MalformedKeystore {
                    reason: "invalid recipient public key".to_string(),
                })?;

        // Encrypt
        let encrypted =
            age::encrypt(&recipient, toml_str.as_bytes()).map_err(|_| {
                AppKeystoreError::MalformedKeystore {
                    reason: "encryption failed".to_string(),
                }
            })?;

        // Zeroize plaintext
        toml_str.zeroize();

        let dir = path.parent().unwrap_or(Path::new("."));

        // Open (or create) the destination file so we can acquire an exclusive
        // advisory lock before the write cycle begins. This serialises
        // concurrent load+save cycles.
        //
        // On the very first write the file may not exist yet; we create it.
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(path)?;
        flock_exclusive(&lock_file)?;
        // We hold the lock for the duration of the tempfile + rename operation.
        // The lock is released when lock_file drops below.

        // Atomic write: tempfile + fsync + rename to avoid torn keystore.
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        std::io::Write::write_all(&mut tmp, &encrypted)?;

        // fsync data to durable storage before the rename makes it visible.
        tmp.as_file().sync_all()?;

        // Set permissions to 0o600 on Unix before rename.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(tmp.path(), perms)?;
        }

        // Atomic rename — on POSIX this is guaranteed atomic within the same
        // filesystem, which the tempfile is (new_in(dir) ensures this).
        tmp.persist(path).map_err(|e| AppKeystoreError::Io(e.error))?;

        // fsync the parent directory so the rename's directory entry is durable.
        let dir_file = std::fs::File::open(dir)?;
        dir_file.sync_all()?;

        // Release the advisory lock by dropping.
        drop(lock_file);

        Ok(())
    }
}
