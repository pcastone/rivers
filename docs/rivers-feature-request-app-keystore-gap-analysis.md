# Rivers App Keystore Gap Analysis

**Related Request:** `docs/rivers-feature-request-app-keystore.md`  
**Date:** 2026-03-27  
**Scope:** Compare the feature request to the current repository state

---

## 1. Summary

The repository is no longer at "feature not started" status. Large parts of the request are already implemented, but the end-to-end handler experience is not complete yet.

If we judge the request against its acceptance criteria, the current repo is best described as:

- **Core engine/CLI/storage work:** mostly done
- **Runtime integration into handlers:** partially done
- **Validation/documentation/polish:** still open

### Quick Build Status

| Capability | Built Status | Notes |
|------------|--------------|-------|
| `[[keystores]]` in `resources.toml` | Built | Runtime config model and parsing are present. |
| `[data.keystore.*]` in `app.toml` | Built | App-side keystore path config is present. |
| Keystore engine (`rivers-keystore-engine`) | Built | Encrypted storage, key lifecycle, AES-256-GCM implemented. |
| CLI (`rivers-keystore`) | Built | `init`, `generate`, `list`, `info`, `delete`, `rotate` are implemented. |
| Startup unlock from Lockbox | Built | Bundle load resolves lockbox alias and unlocks keystore. |
| V8 host APIs (`Rivers.keystore`, `Rivers.crypto`) | Built | Host functions exist in runtime. |
| WASM host APIs (`keystore_*`, `crypto_*`) | Built | Host functions exist in runtime. |
| Handler dispatch wiring (`TaskContextBuilder::keystore`) | Not built end-to-end | Keystore field exists, but normal dispatch paths are not wired consistently. |
| Multi-keystore disambiguation (`"ks/key"`) | Not built | Current handler-side model effectively uses one keystore per task. |
| `riversctl validate` alias + key-reference checks | Not built | Current validation only includes config checks + missing-file warning. |
| End-to-end `riversd` integration tests for handler crypto flow | Not built | Engine-level tests exist, runtime E2E coverage is still missing. |

---

## 2. Landed

- `[[keystores]]` and `[data.keystore.*]` config models exist in the runtime.
- `rivers-keystore-engine` exists with encrypted keystore persistence, AES-256-GCM encrypt/decrypt, key metadata, and rotation support.
- `rivers-keystore` CLI exists with `init`, `generate`, `list`, `info`, `delete`, and `rotate`.
- Startup loading exists: Rivers resolves the Lockbox alias, reads the master key, and unlocks the application keystore during bundle load.
- V8 and WASM host bindings exist for `Rivers.keystore.has/info` and `Rivers.crypto.encrypt/decrypt`.
- The keystore engine itself has strong unit/integration coverage.

---

## 3. Remaining Gaps

| Area | Current State | Gap vs Request |
|------|---------------|----------------|
| **Handler runtime wiring** | The unlocked keystore is stored in `AppContext.keystore_resolver`, and `TaskContext` has a `keystore` field, but the normal request dispatch paths are not populating `TaskContextBuilder::keystore(...)`. | The API surface exists, but handler code will still behave as if no keystore is configured in normal execution paths until the keystore is injected during dispatch. |
| **Multiple keystores per app** | Runtime handler execution carries a single `Option<AppKeystore>` in `TaskContext`. Host APIs accept a plain key name and resolve directly against that one keystore. | The requested `"keystore-name/key-name"` disambiguation model is not implemented yet. Current wiring only supports one effective keystore per task. |
| **Validation depth** | `rivers-runtime` validates config shape and `riversctl validate` warns if the keystore file is missing. | The request called for Lockbox alias resolution checks and handler key-reference validation. Those checks are not implemented in `riversctl validate` today. |
| **Provisioning contract** | Runtime and CLI expect the keystore master key to be an Age identity string. The CLI also requires `RIVERS_KEYSTORE_KEY`. | The related feature request currently shows `openssl rand -hex 32`, which does not match the implementation contract. The operational docs need to be updated to reflect the actual key format and CLI env var. |
| **App scoping key** | The resolver stores keystores under `"{entry_point}:{keystore_name}"`. | The request describes scoping by `appId`. Current code is still scoped by entry point/app name rather than app ID. |
| **End-to-end runtime tests** | Engine-crate crypto tests are present and passing. | There is little to no `riversd` coverage proving bundle load -> keystore unlock -> task dispatch -> handler encrypt/decrypt works end to end. |
| **Error contract** | The implementation returns string errors from host bindings, with generic decrypt failures and specific key-not-found/key-version-not-found handling. | The behavior is close, but the exact typed/named error contract described in the request is not fully aligned with the current surfaced messages. |

---

## 4. Acceptance Criteria Readout

| Acceptance Area | Status | Notes |
|-----------------|--------|-------|
| Resource declaration in `resources.toml` | **Met** | Runtime structs and parsing exist. |
| `[data.keystore.*]` config in `app.toml` | **Met** | Runtime structs and parsing exist. |
| Master key resolved from Lockbox at startup | **Mostly met** | Bundle loading resolves and unlocks keystores. |
| `rivers-keystore` CLI lifecycle commands | **Met** | `init`, `generate`, `list`, `info`, `delete`, `rotate` are implemented. |
| `Rivers.keystore.has/info` from handlers | **Partially met** | Host bindings exist, but dispatch wiring appears incomplete. |
| `Rivers.crypto.encrypt/decrypt` from handlers | **Partially met** | Host bindings and engine support exist, but dispatch wiring appears incomplete. |
| Generic decrypt failures | **Mostly met** | Decrypt failures are intentionally generic in host bindings. |
| Keys never exposed to handlers | **Mostly met** | Current design keeps key bytes in Rust memory. |
| App scoping isolation | **Partially met** | Scoped in runtime, but by entry point rather than `appId`. |
| Encrypted keystore at rest | **Met** | Age-encrypted keystore storage is implemented. |
| `riversctl validate` keystore checks | **Partially met** | File existence warning exists; alias/key-reference checks do not. |
| JS and WASM parity | **Mostly met** | Both engines have host bindings. |
| Rotation preserves older versions | **Met** | Engine and tests support versioned rotation. |

---

## 5. Recommended Next Steps

1. Inject the resolved app keystore into every `TaskContextBuilder` path that dispatches handler code.
2. Decide whether Rivers v1 supports exactly one app keystore per task or the full multi-keystore name-resolution model from the request, then align runtime and docs.
3. Upgrade `riversctl validate` to check Lockbox alias reachability and, if feasible, static handler key references.
4. Fix the provisioning/docs contract so the documented master-key format matches the Age-based implementation.
5. Add at least one `riversd` integration test covering a real handler encrypt/decrypt flow.

---

## 6. Verification Notes

- `cargo test -p rivers-keystore-engine --quiet` passed.
- `cargo test -p rivers-runtime validate_keystores --quiet` did not complete because unrelated feature-gated test imports failed in that crate.
