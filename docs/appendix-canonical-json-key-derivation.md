# Appendix: Canonical JSON & Key Derivation

<!-- SHAPE-3 amendment: shared canonical JSON and key derivation algorithm -->

**Document Type:** Shared Appendix
**Origin:** SHAPE-3 (Cache Key — Canonical JSON Defined)
**Referenced By:** rivers-data-layer-spec.md (DataView cache keys), rivers-polling-views-spec.md (poll state keys), rivers-storage-engine-spec.md (L2 cache keys)

---

## 1. Canonical JSON Serialization

All key derivation in Rivers uses a single canonical JSON serialization algorithm:

1. Collect parameters into a `BTreeMap<String, serde_json::Value>`
   - `BTreeMap` provides deterministic key ordering (lexicographic by key name)
   - This ensures `{a:1, b:2}` and `{b:2, a:1}` produce identical serializations
2. Serialize via `serde_json::to_string(&btreemap)`
   - No pretty-printing, no trailing whitespace
   - serde_json produces deterministic output for the same BTreeMap input

```rust
use std::collections::BTreeMap;
use serde_json;
use sha2::{Sha256, Digest};

fn canonical_json(params: &BTreeMap<String, serde_json::Value>) -> String {
    serde_json::to_string(params).expect("BTreeMap<String, Value> is always serializable")
}
```

---

## 2. Key Derivation

The canonical JSON string is hashed with SHA-256 and hex-encoded:

```rust
fn derive_param_hash(params: &BTreeMap<String, serde_json::Value>) -> String {
    let json = canonical_json(params);
    let mut hasher = Sha256::new();
    hasher.update(json.as_bytes());
    hex::encode(hasher.finalize())
}
```

---

## 3. Key Format

All derived keys follow the format: `{prefix}:{entity}:{sha256_hex}`

| Consumer | Key format | Example |
|---|---|---|
| DataView L1/L2 cache | `cache:views:{view_name}:{param_hash}` | `cache:views:get_order:a1b2c3d4...` |
| Polling loop state | `poll:{view_name}:{param_hash}` | `poll:price_feed:e5f6a7b8...` |
| StorageEngine L2 | Uses DataView cache key directly | `cache:views:get_order:a1b2c3d4...` |

---

## 4. Properties

- **Deterministic:** Same parameters always produce the same key, regardless of insertion order
- **Collision-resistant:** SHA-256 provides negligible collision probability for practical key spaces
- **One algorithm:** All consumers (cache, polling, StorageEngine) use the same `canonical_json` + SHA-256 pipeline
- **No custom serialization:** Relies entirely on `BTreeMap` ordering and `serde_json` — no hand-rolled JSON serializer

---

## 5. Empty Parameters

When the parameter set is empty (`{}`), the canonical JSON is `"{}"` and the SHA-256 hash is the hash of that string. This is a valid key — views with no parameters still produce a deterministic cache/poll key.
