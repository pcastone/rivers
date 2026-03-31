//! Diff strategies and hashing for poll loop change detection.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ── Diff Strategy ───────────────────────────────────────────────

/// Diff strategy for determining whether polled data has changed.
///
/// Per spec: hash, null, or change_detect (CodeComponent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffStrategy {
    /// SHA-256 of canonical JSON — change if hash differs.
    Hash,
    /// Non-empty presence check — change if result is non-null/non-empty.
    Null,
    /// User CodeComponent receives prev + current, decides.
    ChangeDetect,
}

impl DiffStrategy {
    /// Parse a diff strategy from an optional string, defaulting to Hash.
    pub fn from_str_opt(s: Option<&str>) -> Self {
        match s {
            Some(s) if s.eq_ignore_ascii_case("null") => DiffStrategy::Null,
            Some(s) if s.eq_ignore_ascii_case("change_detect") => DiffStrategy::ChangeDetect,
            _ => DiffStrategy::Hash, // default
        }
    }
}

// ── Poll Loop Key ───────────────────────────────────────────────

/// Key for a poll loop instance: `poll:{view_id}:{param_hash}`.
///
/// Per spec: multiple clients with same parameters share one poll loop.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PollLoopKey {
    /// View identifier for this poll loop.
    pub view_id: String,
    /// SHA-256 hash of the query parameters.
    pub param_hash: String,
}

impl PollLoopKey {
    /// Create a new poll loop key from a view ID and query parameters.
    pub fn new(view_id: &str, params: &HashMap<String, String>) -> Self {
        let param_hash = compute_param_hash(params);
        Self {
            view_id: view_id.to_string(),
            param_hash,
        }
    }

    /// Storage key for the poll loop: `poll:{view}:{hash}`.
    pub fn storage_key(&self) -> String {
        format!("poll:{}:{}", self.view_id, self.param_hash)
    }

    /// Storage key for previous poll result: `poll:{view}:{hash}:prev`.
    pub fn storage_key_prev(&self) -> String {
        format!("poll:{}:{}:prev", self.view_id, self.param_hash)
    }

    /// Storage key for poll loop metadata: `poll:{view}:{hash}:meta`.
    pub fn storage_key_meta(&self) -> String {
        format!("poll:{}:{}:meta", self.view_id, self.param_hash)
    }
}

/// Compute a deterministic hash of parameters for deduplication.
pub(crate) fn compute_param_hash(params: &HashMap<String, String>) -> String {
    use sha2::{Digest, Sha256};

    let mut sorted: Vec<(&String, &String)> = params.iter().collect();
    sorted.sort_by_key(|(k, _)| *k);

    let mut hasher = Sha256::new();
    for (k, v) in sorted {
        hasher.update(k.as_bytes());
        hasher.update(b"=");
        hasher.update(v.as_bytes());
        hasher.update(b"&");
    }

    hex::encode(hasher.finalize())
}

// ── Hash Diff ───────────────────────────────────────────────────

/// Compute SHA-256 hash of canonical JSON for hash diff strategy.
///
/// Per spec: canonical = serde_json::to_string (deterministic for same structure).
pub fn compute_data_hash(data: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};

    let canonical = serde_json::to_string(data).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    hex::encode(hasher.finalize())
}

/// Check if data has changed using hash diff strategy.
pub fn hash_diff(prev_hash: Option<&str>, current: &serde_json::Value) -> (bool, String) {
    let current_hash = compute_data_hash(current);
    let changed = match prev_hash {
        Some(prev) => prev != current_hash,
        None => true, // first poll always "changed"
    };
    (changed, current_hash)
}

/// Check if data has changed using null diff strategy.
///
/// Per spec: change if result is non-null and non-empty.
pub fn null_diff(current: &serde_json::Value) -> bool {
    match current {
        serde_json::Value::Null => false,
        serde_json::Value::Array(arr) => !arr.is_empty(),
        serde_json::Value::Object(obj) => !obj.is_empty(),
        serde_json::Value::String(s) => !s.is_empty(),
        _ => true, // numbers, bools are non-null presence
    }
}

// ── Change Detect Diff Strategy (D15) ───────────────────────

/// Result of a JSON diff comparison.
#[derive(Debug, Clone)]
pub struct DiffResult {
    /// Whether any changes were detected.
    pub changed: bool,
    /// Number of added keys/elements.
    pub added_count: usize,
    /// Number of removed keys/elements.
    pub removed_count: usize,
    /// Number of modified values.
    pub modified_count: usize,
}

/// Diff strategy for polling views.
///
/// Per SHAPE-20: emits diagnostic events on diff operations.
/// Compares two JSON values and reports changes at the top level.
pub fn compute_diff(
    prev: &serde_json::Value,
    current: &serde_json::Value,
) -> DiffResult {
    if prev == current {
        return DiffResult {
            changed: false,
            added_count: 0,
            removed_count: 0,
            modified_count: 0,
        };
    }

    match (prev, current) {
        (serde_json::Value::Object(prev_obj), serde_json::Value::Object(curr_obj)) => {
            let mut added = 0usize;
            let mut removed = 0usize;
            let mut modified = 0usize;

            // Check for added and modified keys
            for (key, curr_val) in curr_obj {
                match prev_obj.get(key) {
                    Some(prev_val) => {
                        if prev_val != curr_val {
                            modified += 1;
                        }
                    }
                    None => {
                        added += 1;
                    }
                }
            }

            // Check for removed keys
            for key in prev_obj.keys() {
                if !curr_obj.contains_key(key) {
                    removed += 1;
                }
            }

            DiffResult {
                changed: added > 0 || removed > 0 || modified > 0,
                added_count: added,
                removed_count: removed,
                modified_count: modified,
            }
        }
        (serde_json::Value::Array(prev_arr), serde_json::Value::Array(curr_arr)) => {
            let prev_len = prev_arr.len();
            let curr_len = curr_arr.len();

            let mut modified = 0usize;
            let common_len = prev_len.min(curr_len);

            for i in 0..common_len {
                if prev_arr[i] != curr_arr[i] {
                    modified += 1;
                }
            }

            let added = if curr_len > prev_len {
                curr_len - prev_len
            } else {
                0
            };
            let removed = if prev_len > curr_len {
                prev_len - curr_len
            } else {
                0
            };

            DiffResult {
                changed: added > 0 || removed > 0 || modified > 0,
                added_count: added,
                removed_count: removed,
                modified_count: modified,
            }
        }
        _ => {
            // Different types or scalar change
            DiffResult {
                changed: true,
                added_count: 0,
                removed_count: 0,
                modified_count: 1,
            }
        }
    }
}
