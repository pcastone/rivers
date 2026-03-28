# Tasks — ExecDriver Gap Analysis Fixes

**Goal:** Fix 11 gaps found in ExecDriver plugin gap analysis. Ranges from blocking (missing runtime logging) to minor (error categorization).

**Branch:** `feature/app-keystore`

---

## Blocking Gaps

- [x] **G1** Runtime logging in `connection.rs` — add tracing spans for command start/success/failure/integrity/concurrency/timeout/overflow (spec S15)
- [x] **G2** `run_as_user` not resolved at startup via `getpwnam` in `config.rs` (spec S4.2)
- [x] **G3** `working_directory` not validated at startup in `config.rs` (spec S4.2)

## Non-Blocking Gaps

- [x] **G4** No executable permission check at startup in `config.rs` (spec S5.1)
- [x] **G5** `env_clear=false` doesn't log WARN at startup in `connection.rs` (spec S11.2, S15.1)
- [x] **G6** `riversctl exec list` stub in `main.rs` (spec S17.4)
- [x] **G7** No change needed — per-file verify approach is correct
- [x] **G8** No change needed — rows[0] approach is correct (SDK has no raw_value)
- [x] **G9** No change needed — implementation correctly uses `every:N`
- [x] **G10** `both` mode removes `stdin_key` before interpolation in `executor.rs`
- [x] **G11** Spawn failure uses `Internal` not `Query` in `executor.rs`

---

## Validation

```bash
cargo test -p rivers-plugin-exec  # 103 tests pass (95 unit + 8 integration)
cargo build -p riversctl           # compiles clean
```
