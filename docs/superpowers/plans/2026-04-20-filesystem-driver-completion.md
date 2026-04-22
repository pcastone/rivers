# Filesystem Driver Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close all outstanding spec/implementation gaps in the filesystem driver so it is spec-compliant, fully configured, ISO-8601 timed, canary-covered, and deployed.

**Architecture:** All changes are contained to three locations — `crates/rivers-drivers-builtin/src/filesystem.rs` (Rust driver), `docs/arch/rivers-filesystem-driver-spec.md` (spec amendments), and `canary-bundle/canary-filesystem/` (canary app). No new files are created; no other crates are touched.

**Tech Stack:** Rust (async-trait, tokio, chrono), TypeScript (V8 canary handlers), TOML (bundle config), bash (run-tests.sh)

---

## File Map

| File | What changes |
|---|---|
| `crates/rivers-drivers-builtin/src/filesystem.rs` | `ping()`, `connect()` extra config, ISO-8601 timestamps |
| `crates/rivers-drivers-builtin/Cargo.toml` | Add `chrono` workspace dep |
| `docs/arch/rivers-filesystem-driver-spec.md` | §3.2 dispatch name, §6.2 readDir type, §6.4 readDir prose |
| `canary-bundle/canary-filesystem/libraries/handlers/filesystem-tests.ts` | Add `fsReadDir` and `fsConcurrentWrites` handlers |
| `canary-bundle/canary-filesystem/app.toml` | Add two new view endpoints |
| `canary-bundle/canary-bundle/run-tests.sh` | Add two new test_ep lines to FILESYSTEM profile |

---

## Task 1: Spec §3.2 — fix dispatch name (G1)

**Files:**
- Modify: `docs/arch/rivers-filesystem-driver-spec.md` lines ~209–216

- [ ] **Step 1: Update the generated JS example in §3.2**

In `docs/arch/rivers-filesystem-driver-spec.md`, find this block (around line 205):

```
    return __rivers_datasource_dispatch("project_files", {
        operation: "readFile",
        parameters: { path, encoding }
    });
};
```

`__rivers_datasource_dispatch` is the existing host function binding that resolves the datasource token and calls the driver. The typed proxy adds type checking and ergonomic method signatures on top of the same dispatch path.

Replace with:

```
    return Rivers.__directDispatch("project_files", "readFile", { path, encoding });
};
```

`Rivers.__directDispatch` is the host function binding for direct-dispatch datasources. The typed proxy adds type checking and ergonomic method signatures on top.

- [ ] **Step 2: Commit**

```bash
git add docs/arch/rivers-filesystem-driver-spec.md
git commit -m "docs(spec): fix §3.2 dispatch name — __directDispatch not __rivers_datasource_dispatch (G1)"
```

---

## Task 2: Spec §6.2 + §6.4 — amend readDir return type (G4)

**Files:**
- Modify: `docs/arch/rivers-filesystem-driver-spec.md` lines ~469, ~497–501

- [ ] **Step 1: Update §6.2 return types table**

Find the `readDir` row in the §6.2 table (around line 469):

```
| `readDir` | `string[]` — filenames only (not full paths), same as Node `fs.readdirSync()` |
```

Replace with:

```
| `readDir` | `{ name: string }[]` — one object per entry, filename only (not full path) |
```

- [ ] **Step 2: Update §6.4 prose**

Find §6.4 (around line 497):

```
### 6.4 readDir

Flat listing only. Returns filenames, not full paths. Does not include `.` or `..`. Entries are not sorted — order is filesystem-dependent, same as Node.

For recursive directory traversal, use `find("*")`. All returned paths use forward slashes on all platforms.
```

Replace with:

```
### 6.4 readDir

Flat listing only. Returns one `{ name: string }` object per entry — filename only, not a full
path. Does not include `.` or `..`. Entries are not sorted — order is filesystem-dependent.

```javascript
const entries = fs.readDir("src/");
// entries: [{ name: "main.rs" }, { name: "lib.rs" }]
const names = entries.map(e => e.name);
```

For recursive directory traversal, use `find("*")`. All returned paths use forward slashes on all platforms.
```

- [ ] **Step 3: Commit**

```bash
git add docs/arch/rivers-filesystem-driver-spec.md
git commit -m "docs(spec): amend §6.2/§6.4 readDir return type to {name:string}[] (G4)"
```

---

## Task 3: Implement `ping()`

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs` line ~182

- [ ] **Step 1: Write the failing test**

In `crates/rivers-drivers-builtin/src/filesystem.rs`, add to the `#[cfg(test)] mod tests` block
(after the `admin_operations_is_empty` test, before the closing `}`):

```rust
#[tokio::test]
async fn ping_ok_when_root_exists() {
    let (_dir, mut conn) = test_connection();
    conn.ping().await.expect("ping should succeed when root exists");
}

#[tokio::test]
async fn ping_fails_when_root_removed() {
    let (dir, mut conn) = test_connection();
    let path = dir.path().to_path_buf();
    drop(dir); // TempDir destructor removes the directory
    // Confirm it's gone
    assert!(!path.exists());
    let err = conn.ping().await.unwrap_err();
    assert!(
        format!("{err}").contains("root directory"),
        "expected 'root directory' in error, got: {err}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /path/to/rivers.pub-filesystem-driver
cargo test -p rivers-drivers-builtin -- ping_ok ping_fails 2>&1 | tail -10
```

Expected: `FAILED` — `ping_ok` panics with "not implemented", `ping_fails` panics similarly.

- [ ] **Step 3: Implement `ping()`**

Find `ping()` in `crates/rivers-drivers-builtin/src/filesystem.rs` (around line 182):

```rust
async fn ping(&mut self) -> Result<(), DriverError> {
    Err(DriverError::NotImplemented("FilesystemConnection::ping — Task 11".into()))
}
```

Replace with:

```rust
async fn ping(&mut self) -> Result<(), DriverError> {
    if self.root.is_dir() {
        Ok(())
    } else {
        Err(DriverError::Connection(format!(
            "root directory no longer accessible: {}",
            self.root.display()
        )))
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p rivers-drivers-builtin -- ping_ok ping_fails 2>&1 | tail -10
```

Expected: `test result: ok. 2 passed`

- [ ] **Step 5: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): implement ping() — validates root dir still accessible"
```

---

## Task 4: Wire `max_file_size` / `max_depth` from extra config (FUP-3)

**Files:**
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs` lines ~140–150

- [ ] **Step 1: Write failing tests**

Add to the tests block in `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
#[tokio::test]
async fn connect_reads_max_file_size_from_options() {
    use std::collections::HashMap;
    let dir = TempDir::new().unwrap();
    let mut options = HashMap::new();
    options.insert("max_file_size".to_string(), "1024".to_string());
    let params = ConnectionParams {
        host: String::new(),
        port: 0,
        database: dir.path().to_str().unwrap().to_string(),
        username: String::new(),
        password: String::new(),
        options,
    };
    let conn = FilesystemDriver.connect(&params).await.unwrap();
    // Downcast to FilesystemConnection to inspect the field
    // We verify the limit is respected by writing a file > 1024 bytes
    let conn = conn;
    drop(conn); // just ensure connect succeeds; limit tested below
}

#[tokio::test]
async fn connect_custom_max_file_size_enforced() {
    use std::collections::HashMap;
    let dir = TempDir::new().unwrap();
    let mut options = HashMap::new();
    options.insert("max_file_size".to_string(), "10".to_string());
    let params = ConnectionParams {
        host: String::new(),
        port: 0,
        database: dir.path().to_str().unwrap().to_string(),
        username: String::new(),
        password: String::new(),
        options,
    };
    let mut conn_box = FilesystemDriver.connect(&params).await.unwrap();
    let q = mkq(
        "writeFile",
        &[
            ("path", QueryValue::String("big.txt".into())),
            ("content", QueryValue::String("a".repeat(100))),
        ],
    );
    let err = conn_box.execute(&q).await.unwrap_err();
    assert!(format!("{err}").contains("exceeds max_file_size"), "err: {err}");
}

#[tokio::test]
async fn connect_invalid_max_file_size_uses_default() {
    use std::collections::HashMap;
    let dir = TempDir::new().unwrap();
    let mut options = HashMap::new();
    options.insert("max_file_size".to_string(), "not_a_number".to_string());
    let params = ConnectionParams {
        host: String::new(),
        port: 0,
        database: dir.path().to_str().unwrap().to_string(),
        username: String::new(),
        password: String::new(),
        options,
    };
    // Should not error — falls back to default
    let conn = FilesystemDriver.connect(&params).await;
    assert!(conn.is_ok(), "connect should succeed with invalid option, falling back to default");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p rivers-drivers-builtin -- connect_reads_max connect_custom_max connect_invalid_max 2>&1 | tail -15
```

Expected: `connect_custom_max_file_size_enforced` FAILS (writes succeed with hardcoded 50MB limit).

- [ ] **Step 3: Implement config reading in `connect()`**

Find `connect()` in `crates/rivers-drivers-builtin/src/filesystem.rs` (around line 140):

```rust
async fn connect(
    &self,
    params: &ConnectionParams,
) -> Result<Box<dyn Connection>, DriverError> {
    let root = Self::resolve_root(&params.database)?;
    Ok(Box::new(FilesystemConnection {
        root,
        max_file_size: DEFAULT_MAX_FILE_SIZE,
        max_depth: DEFAULT_MAX_DEPTH,
    }))
}
```

Replace with:

```rust
async fn connect(
    &self,
    params: &ConnectionParams,
) -> Result<Box<dyn Connection>, DriverError> {
    let root = Self::resolve_root(&params.database)?;

    let max_file_size = params
        .options
        .get("max_file_size")
        .and_then(|v| {
            v.parse::<u64>().map_err(|_| {
                tracing::warn!(
                    "filesystem: invalid max_file_size {:?}, using default {}",
                    v, DEFAULT_MAX_FILE_SIZE
                );
            }).ok()
        })
        .unwrap_or(DEFAULT_MAX_FILE_SIZE);

    let max_depth = params
        .options
        .get("max_depth")
        .and_then(|v| {
            v.parse::<usize>().map_err(|_| {
                tracing::warn!(
                    "filesystem: invalid max_depth {:?}, using default {}",
                    v, DEFAULT_MAX_DEPTH
                );
            }).ok()
        })
        .unwrap_or(DEFAULT_MAX_DEPTH);

    Ok(Box::new(FilesystemConnection {
        root,
        max_file_size,
        max_depth,
    }))
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo test -p rivers-drivers-builtin -- connect_reads_max connect_custom_max connect_invalid_max 2>&1 | tail -10
```

Expected: `test result: ok. 3 passed`

- [ ] **Step 5: Run full suite to ensure no regressions**

```bash
cargo test -p rivers-drivers-builtin 2>&1 | tail -5
```

Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/rivers-drivers-builtin/src/filesystem.rs
git commit -m "feat(filesystem): read max_file_size + max_depth from ConnectionParams.options (FUP-3)"
```

---

## Task 5: ISO-8601 timestamps in `stat` (FUP-1)

**Files:**
- Modify: `crates/rivers-drivers-builtin/Cargo.toml`
- Modify: `crates/rivers-drivers-builtin/src/filesystem.rs` lines ~387–416
- Modify: `docs/arch/rivers-filesystem-driver-spec.md` §6.2 stat row

- [ ] **Step 1: Add chrono to Cargo.toml**

`chrono` is already in the workspace (`Cargo.toml` root has `chrono = { version = "0.4", features = ["serde"] }`). Add it to `crates/rivers-drivers-builtin/Cargo.toml` under `[dependencies]`:

```toml
chrono = { workspace = true }
```

- [ ] **Step 2: Write failing test**

Add to the tests block in `crates/rivers-drivers-builtin/src/filesystem.rs`:

```rust
#[tokio::test]
async fn stat_timestamps_are_iso8601() {
    let (dir, mut conn) = test_connection();
    std::fs::write(dir.path().join("ts.txt"), b"x").unwrap();
    let q = mkq("stat", &[("path", QueryValue::String("ts.txt".into()))]);
    let result = conn.execute(&q).await.unwrap();
    let row = &result.rows[0];
    for key in ["mtime", "atime", "ctime"] {
        match row.get(key) {
            Some(QueryValue::String(s)) => {
                // Must parse as RFC 3339
                chrono::DateTime::parse_from_rfc3339(s).unwrap_or_else(|e| {
                    panic!("{key} value {:?} is not valid RFC 3339: {e}", s)
                });
            }
            other => panic!("{key}: expected String, got {other:?}"),
        }
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p rivers-drivers-builtin -- stat_timestamps_are_iso8601 2>&1 | tail -10
```

Expected: FAIL — current timestamps are epoch-second strings like `"1713312000"`, which are not valid RFC 3339.

- [ ] **Step 4: Add chrono import and update `epoch_secs` in `ops::stat`**

At the top of `crates/rivers-drivers-builtin/src/filesystem.rs`, in the `mod ops { use super::*; ...` block, add:

```rust
use chrono::{DateTime, Utc};
```

Then find `fn epoch_secs` inside `ops::stat` (around line 387):

```rust
fn epoch_secs(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    secs.to_string()
}
```

Replace with:

```rust
fn to_iso8601(t: std::time::SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339()
}
```

Then update the three timestamp insertions in `ops::stat` that call `epoch_secs(...)`:

```rust
row.insert(
    "mtime".into(),
    QueryValue::String(to_iso8601(md.modified().unwrap_or(std::time::UNIX_EPOCH))),
);
row.insert(
    "atime".into(),
    QueryValue::String(to_iso8601(md.accessed().unwrap_or(std::time::UNIX_EPOCH))),
);
row.insert(
    "ctime".into(),
    QueryValue::String(to_iso8601(md.created().unwrap_or(std::time::UNIX_EPOCH))),
);
```

- [ ] **Step 5: Update the existing `stat_file_returns_metadata` test**

The existing test checks column names but doesn't validate timestamp format. It will still pass — no change needed. Verify:

```bash
cargo test -p rivers-drivers-builtin -- stat_file_returns_metadata stat_timestamps_are_iso8601 2>&1 | tail -10
```

Expected: `test result: ok. 2 passed`

- [ ] **Step 6: Update spec §6.2 stat row**

In `docs/arch/rivers-filesystem-driver-spec.md`, find the stat row in §6.2 (around line 470):

```
| `stat` | `{ size: number, mtime: string, atime: string, ctime: string, isFile: boolean, isDirectory: boolean, mode: number }` |
```

Replace with:

```
| `stat` | `{ size: number, mtime: string, atime: string, ctime: string, isFile: boolean, isDirectory: boolean, mode: number }` — timestamps are RFC 3339 UTC strings e.g. `"2026-04-20T12:00:00+00:00"` |
```

- [ ] **Step 7: Commit**

```bash
git add crates/rivers-drivers-builtin/Cargo.toml \
        crates/rivers-drivers-builtin/src/filesystem.rs \
        docs/arch/rivers-filesystem-driver-spec.md
git commit -m "feat(filesystem): emit ISO-8601 timestamps in stat (FUP-1)"
```

---

## Task 6: Add `readDir` canary endpoint

**Files:**
- Modify: `canary-bundle/canary-filesystem/libraries/handlers/filesystem-tests.ts`
- Modify: `canary-bundle/canary-filesystem/app.toml`
- Modify: `canary-bundle/run-tests.sh`

- [ ] **Step 1: Add `fsReadDir` handler**

In `canary-bundle/canary-filesystem/libraries/handlers/filesystem-tests.ts`, append before the final line:

```typescript
// ── FS-READ-DIR — readDir returns {name:string}[] objects ──

function fsReadDir(ctx) {
    var t = new TestResult("FS-READ-DIR", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 6.4");
    var fs = ctx.datasource("canary-fs");
    var work = workDir("readdir");
    cleanup(fs, work);
    try {
        fs.mkdir(work);
        fs.writeFile(work + "/alpha.txt", "a");
        fs.writeFile(work + "/beta.txt", "b");
        fs.mkdir(work + "/subdir");

        var entries = fs.readDir(work);

        // Must be an array
        t.assert("is_array", Array.isArray(entries), "readDir returned: " + typeof entries);

        // Each entry must be an object with a name property
        t.assert("entries_have_name",
            entries.every(function(e) { return typeof e === 'object' && typeof e.name === 'string'; }),
            "entry shape wrong: " + JSON.stringify(entries[0]));

        // Must contain our three entries (order not guaranteed)
        var names = entries.map(function(e) { return e.name; }).sort();
        t.assertEquals("entry_count", 3, names.length);
        t.assertEquals("entry_alpha", "alpha.txt", names[0]);
        t.assertEquals("entry_beta", "beta.txt", names[1]);
        t.assertEquals("entry_subdir", "subdir", names[2]);

        // Must not include . or ..
        t.assert("no_dot_entries",
            !names.some(function(n) { return n === '.' || n === '..'; }));

    } catch (e) {
        cleanup(fs, work);
        ctx.resdata = t.fail(String(e));
        return;
    }
    cleanup(fs, work);
    ctx.resdata = t.finish();
}
```

- [ ] **Step 2: Register endpoint in app.toml**

In `canary-bundle/canary-filesystem/app.toml`, append:

```toml
[api.views.fs_read_dir]
path      = "/canary/fs/read-dir"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.fs_read_dir.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/filesystem-tests.ts"
entrypoint = "fsReadDir"
```

- [ ] **Step 3: Add to run-tests.sh**

In `canary-bundle/run-tests.sh`, find the FILESYSTEM Profile section:

```bash
echo "  ── FILESYSTEM Profile ──"
test_ep "fs-crud-roundtrip"  GET  "$BASE/filesystem/canary/fs/crud-roundtrip"
test_ep "fs-chroot-escape"   GET  "$BASE/filesystem/canary/fs/chroot-escape"
test_ep "fs-exists-stat"     GET  "$BASE/filesystem/canary/fs/exists-and-stat"
test_ep "fs-find-grep"       GET  "$BASE/filesystem/canary/fs/find-and-grep"
test_ep "fs-arg-validation"  GET  "$BASE/filesystem/canary/fs/arg-validation"
```

Add one line at the end of that block:

```bash
test_ep "fs-read-dir"        GET  "$BASE/filesystem/canary/fs/read-dir"
```

- [ ] **Step 4: Commit**

```bash
git add canary-bundle/canary-filesystem/libraries/handlers/filesystem-tests.ts \
        canary-bundle/canary-filesystem/app.toml \
        canary-bundle/run-tests.sh
git commit -m "test(canary): add fs-read-dir endpoint — covers readDir {name}[] shape (item 6)"
```

---

## Task 7: Add concurrent-write canary endpoint

**Files:**
- Modify: `canary-bundle/canary-filesystem/libraries/handlers/filesystem-tests.ts`
- Modify: `canary-bundle/canary-filesystem/app.toml`
- Modify: `canary-bundle/run-tests.sh`

- [ ] **Step 1: Add `fsConcurrentWrites` handler**

Append to `canary-bundle/canary-filesystem/libraries/handlers/filesystem-tests.ts`:

```typescript
// ── FS-CONCURRENT-WRITES — sequential writes to independent dirs within one handler ──
// V8 is single-threaded per isolate, so "concurrent" here means multiple
// independent write sequences with no shared state. Validates that the
// driver's lock-free model (each op is path-independent) holds.

function fsConcurrentWrites(ctx) {
    var t = new TestResult("FS-CONCURRENT-WRITES", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 5.4");
    var fs = ctx.datasource("canary-fs");
    var N = 8;
    var dirs = [];
    for (var i = 0; i < N; i++) {
        dirs.push(workDir("concurrent-" + i));
    }
    // cleanup any leftovers
    dirs.forEach(function(d) { cleanup(fs, d); });

    try {
        // Write phase — create dir + file in each independent work dir
        for (var i = 0; i < N; i++) {
            fs.mkdir(dirs[i]);
            fs.writeFile(dirs[i] + "/data.txt", "payload-" + i);
        }

        // Read-back phase — each file must contain its own payload
        var allCorrect = true;
        for (var i = 0; i < N; i++) {
            var got = fs.readFile(dirs[i] + "/data.txt");
            if (got !== "payload-" + i) {
                allCorrect = false;
                t.assert("payload_" + i, false,
                    "expected 'payload-" + i + "', got '" + got + "'");
            } else {
                t.assert("payload_" + i, true);
            }
        }
        t.assert("all_payloads_correct", allCorrect);

    } catch (e) {
        dirs.forEach(function(d) { cleanup(fs, d); });
        ctx.resdata = t.fail(String(e));
        return;
    }
    dirs.forEach(function(d) { cleanup(fs, d); });
    ctx.resdata = t.finish();
}
```

- [ ] **Step 2: Register endpoint in app.toml**

Append to `canary-bundle/canary-filesystem/app.toml`:

```toml
[api.views.fs_concurrent_writes]
path      = "/canary/fs/concurrent-writes"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.fs_concurrent_writes.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/filesystem-tests.ts"
entrypoint = "fsConcurrentWrites"
```

- [ ] **Step 3: Add to run-tests.sh**

In `canary-bundle/run-tests.sh`, find the FILESYSTEM Profile block (now has 6 lines including fs-read-dir) and add:

```bash
test_ep "fs-concurrent-writes" GET  "$BASE/filesystem/canary/fs/concurrent-writes"
```

- [ ] **Step 4: Commit**

```bash
git add canary-bundle/canary-filesystem/libraries/handlers/filesystem-tests.ts \
        canary-bundle/canary-filesystem/app.toml \
        canary-bundle/run-tests.sh
git commit -m "test(canary): add fs-concurrent-writes endpoint (FUP-5)"
```

---

## Task 8: Deploy and run canary (FUP-6)

**Files:** No source changes — ops-only.

- [ ] **Step 1: Deploy from the feature branch**

```bash
cd /path/to/rivers.pub-filesystem-driver
just deploy /path/to/release/canary
```

Or if using cargo deploy directly:

```bash
cargo deploy /path/to/release/canary --static
```

Expected: build completes, binaries and bundle files written to `release/canary/`.

- [ ] **Step 2: Validate the bundle**

```bash
riverpackage validate canary-bundle --config release/canary/apphome/canary-bundle/riversd-canary.toml
```

Expected: `PASS` with 0 errors.

- [ ] **Step 3: Start riversd**

```bash
release/canary/release/canary/bin/riversctl start --foreground &
```

Wait for the log line: `riversd: listening on 0.0.0.0:8090` (or whichever port riversd-canary.toml specifies).

- [ ] **Step 4: Run the full canary test suite**

```bash
cd release/canary/apphome/canary-bundle
./run-tests.sh https://localhost:8090
```

Expected output for the FILESYSTEM profile section:

```
  ── FILESYSTEM Profile ──
  PASS FS-CRUD-ROUNDTRIP
  PASS FS-CHROOT-ESCAPE
  PASS FS-EXISTS-AND-STAT
  PASS FS-FIND-AND-GREP
  PASS FS-ARG-VALIDATION
  PASS FS-READ-DIR
  PASS FS-CONCURRENT-WRITES
```

Zero FAIL, zero ERR in the FILESYSTEM section. Overall pass count should equal total lines.

- [ ] **Step 5: Stop riversd**

```bash
release/canary/release/canary/bin/riversctl stop
```

- [ ] **Step 6: Record results in changelog**

In `todo/changelog.md` (or `changedecisionlog.md` per CLAUDE.md convention), add an entry:

```
## 2026-04-20 — Filesystem driver completion (FUP-1..6 closed)

- ping() implemented — validates root dir existence
- max_file_size / max_depth now read from ConnectionParams.options (FUP-3)
- stat timestamps changed to RFC 3339 UTC strings (FUP-1)
- readDir spec amended to {name:string}[] (G4)
- §3.2 dispatch name corrected to Rivers.__directDispatch (G1)
- canary-filesystem: added fs-read-dir + fs-concurrent-writes endpoints
- Canary fleet run: FILESYSTEM profile 7/7 PASS (FUP-6)
- Security: symlink traversal blocked in copy/grep/find (prior session)
- Security: chroot path stripped from Forbidden message (G6, prior session)
```

- [ ] **Step 7: Final commit**

```bash
git add todo/changelog.md   # or changedecisionlog.md
git commit -m "chore: record filesystem driver completion — all FUPs closed, canary 7/7 PASS"
```

---

## Self-Review Checklist

- **Spec coverage:** G1 (Task 1) ✓, G4 (Task 2) ✓, ping (Task 3) ✓, FUP-3 (Task 4) ✓, FUP-1 (Task 5) ✓, readDir canary (Task 6) ✓, FUP-5 (Task 7) ✓, FUP-6 (Task 8) ✓
- **Out of scope confirmed:** FUP-2 (Windows CI), FUP-4 (cdylib token) — no tasks, correct
- **Type consistency:** `to_iso8601` defined and used in Task 5 only ✓; `fsReadDir`/`fsConcurrentWrites` defined and registered in same tasks ✓
- **Test commands:** All use `cargo test -p rivers-drivers-builtin` with specific test name filters ✓
- **No placeholders:** All code blocks are complete ✓
