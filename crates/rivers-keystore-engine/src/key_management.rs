//! Key management — generate, rotate, delete, query, and decode keys.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::Utc;
use rand::RngCore;
use zeroize::{Zeroize, Zeroizing};

use crate::types::*;

// ── Key management ──────────────────────────────────────────────────

impl AppKeystore {
    /// Generate a new named key with the given type.
    ///
    /// Only `"aes-256"` is supported. Returns `InvalidKeyType` for
    /// anything else. Returns `DuplicateKey` if a key with this name
    /// already exists.
    pub fn generate_key(
        &mut self,
        name: &str,
        key_type: &str,
    ) -> Result<&AppKeystoreKey, AppKeystoreError> {
        // Validate key type
        if key_type != SUPPORTED_KEY_TYPE {
            return Err(AppKeystoreError::InvalidKeyType {
                expected: SUPPORTED_KEY_TYPE.to_string(),
                got: key_type.to_string(),
            });
        }

        // Check for duplicate
        if self.has_key(name) {
            return Err(AppKeystoreError::DuplicateKey {
                name: name.to_string(),
            });
        }

        // Generate 32 random bytes
        let mut raw_key = vec![0u8; AES_256_KEY_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut raw_key);

        let key_material = BASE64.encode(&raw_key);

        // Zeroize raw bytes
        raw_key.zeroize();

        let now = Utc::now();

        let version = KeyVersion {
            version: 1,
            key_material,
            created: now,
        };

        let key = AppKeystoreKey {
            name: name.to_string(),
            key_type: key_type.to_string(),
            current_version: 1,
            created: now,
            updated: now,
            versions: vec![version],
        };

        self.keys.push(key);
        Ok(self.keys.last().unwrap())
    }

    /// Find a key by name.
    pub fn get_key(&self, name: &str) -> Option<&AppKeystoreKey> {
        self.keys.iter().find(|k| k.name == name)
    }

    /// Find a specific version of a key.
    pub fn get_key_version(
        &self,
        name: &str,
        version: u32,
    ) -> Result<&KeyVersion, AppKeystoreError> {
        let key = self
            .get_key(name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        key.versions
            .iter()
            .find(|v| v.version == version)
            .ok_or_else(|| AppKeystoreError::KeyVersionNotFound {
                name: name.to_string(),
                version,
            })
    }

    /// Check if a key with the given name exists.
    pub fn has_key(&self, name: &str) -> bool {
        self.keys.iter().any(|k| k.name == name)
    }

    /// Return metadata for a key without exposing raw key bytes.
    pub fn key_info(&self, name: &str) -> Result<KeyInfo, AppKeystoreError> {
        let key = self
            .get_key(name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        Ok(KeyInfo {
            name: key.name.clone(),
            key_type: key.key_type.clone(),
            current_version: key.current_version,
            version_count: key.versions.len(),
            created: key.created,
            updated: key.updated,
        })
    }

    /// List metadata for all keys.
    pub fn list_keys(&self) -> Vec<KeyInfo> {
        self.keys
            .iter()
            .map(|key| KeyInfo {
                name: key.name.clone(),
                key_type: key.key_type.clone(),
                current_version: key.current_version,
                version_count: key.versions.len(),
                created: key.created,
                updated: key.updated,
            })
            .collect()
    }

    /// Rotate a key — generates a new version N+1, keeps old versions
    /// for decryption. Returns the new version number.
    pub fn rotate_key(&mut self, name: &str) -> Result<u32, AppKeystoreError> {
        let key = self
            .keys
            .iter_mut()
            .find(|k| k.name == name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        // Generate new key material
        let mut raw_key = vec![0u8; AES_256_KEY_SIZE];
        rand::rngs::OsRng.fill_bytes(&mut raw_key);
        let key_material = BASE64.encode(&raw_key);
        raw_key.zeroize();

        let new_version = key.current_version.checked_add(1).ok_or_else(|| {
            AppKeystoreError::MalformedKeystore {
                reason: format!(
                    "key '{}' version counter overflow (current_version = {})",
                    name, key.current_version
                ),
            }
        })?;
        let now = Utc::now();

        key.versions.push(KeyVersion {
            version: new_version,
            key_material,
            created: now,
        });

        key.current_version = new_version;
        key.updated = now;

        Ok(new_version)
    }

    /// Delete a key by name. Returns `KeyNotFound` if not found.
    pub fn delete_key(&mut self, name: &str) -> Result<(), AppKeystoreError> {
        let pos = self
            .keys
            .iter()
            .position(|k| k.name == name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        // Remove triggers Drop which triggers Zeroize
        self.keys.remove(pos);
        Ok(())
    }

    /// Decode the current version's key material into raw bytes.
    ///
    /// The returned bytes are wrapped in `Zeroizing` and will be zeroized on drop.
    pub fn current_key_bytes(&self, name: &str) -> Result<Zeroizing<Vec<u8>>, AppKeystoreError> {
        let key = self
            .get_key(name)
            .ok_or_else(|| AppKeystoreError::KeyNotFound {
                name: name.to_string(),
            })?;

        let current = key
            .versions
            .iter()
            .find(|v| v.version == key.current_version)
            .ok_or_else(|| AppKeystoreError::KeyVersionNotFound {
                name: name.to_string(),
                version: key.current_version,
            })?;

        let bytes =
            BASE64
                .decode(&current.key_material)
                .map_err(|_| AppKeystoreError::MalformedKeystore {
                    reason: format!(
                        "key '{}' version {} has invalid base64 key material",
                        name, current.version
                    ),
                })?;

        if bytes.len() != AES_256_KEY_SIZE {
            return Err(AppKeystoreError::InvalidKeyLength {
                expected: AES_256_KEY_SIZE,
                got: bytes.len(),
            });
        }

        Ok(Zeroizing::new(bytes))
    }

    /// Decode a specific version's key material into raw bytes.
    ///
    /// The returned bytes are wrapped in `Zeroizing` and will be zeroized on drop.
    pub fn versioned_key_bytes(
        &self,
        name: &str,
        version: u32,
    ) -> Result<Zeroizing<Vec<u8>>, AppKeystoreError> {
        let kv = self.get_key_version(name, version)?;

        let bytes =
            BASE64
                .decode(&kv.key_material)
                .map_err(|_| AppKeystoreError::MalformedKeystore {
                    reason: format!(
                        "key '{}' version {} has invalid base64 key material",
                        name, version
                    ),
                })?;

        if bytes.len() != AES_256_KEY_SIZE {
            return Err(AppKeystoreError::InvalidKeyLength {
                expected: AES_256_KEY_SIZE,
                got: bytes.len(),
            });
        }

        Ok(Zeroizing::new(bytes))
    }
}
