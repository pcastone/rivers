# Rivers Filesystem Driver & Operation Descriptor Specification

**Document Type:** Implementation Specification  
**Scope:** OperationDescriptor framework addition to Driver SDK, `filesystem` built-in driver, V8 typed proxy codegen  
**Status:** Design / Pre-Implementation  
**Patches:** `rivers-driver-spec.md` (§2, §8), `rivers-data-layer-spec.md` (§2.5, §8), `rivers-processpool-runtime-spec-v2.md` (§5.2, §10), `rivers-feature-inventory.md` (§6.1)  
**Depends On:** Epic 6 (Driver SDK), Epic 9 (ProcessPool), Epic 19 (App Bundles)

---

## Table of Contents

1. [Design Rationale](#1-design-rationale)
2. [OperationDescriptor — Framework Addition](#2-operationdescriptor--framework-addition)
3. [V8 Typed Proxy Codegen](#3-v8-typed-proxy-codegen)
4. [Filesystem Driver](#4-filesystem-driver)
5. [Chroot Security Model](#5-chroot-security-model)
6. [Operation Catalog](#6-operation-catalog)
7. [Execution Path — Direct I/O](#7-execution-path--direct-io)
8. [Configuration Reference](#8-configuration-reference)
9. [JS Handler API](#9-js-handler-api)
10. [Error Model](#10-error-model)
11. [Admin Operations](#11-admin-operations)
12. [Implementation Notes](#12-implementation-notes)

---

## 1. Design Rationale

### 1.1 Problem

The existing driver contract (`DatabaseDriver` + `Connection`) normalizes all operations through `Query` structs and `QueryResult` responses. This works for database drivers where SQL or operation strings are the native API. It does not produce ergonomic JS handler code for drivers with discrete, typed operations — filesystem, Redis, MongoDB, and others whose native APIs are method calls, not query strings.

The current workaround is `ctx.datasource("name").fromQuery(...)` — the pseudo DataView builder. For a filesystem operation, this forces the developer into:

```javascript
// Unacceptable
const result = ctx.datasource("files").fromQuery("readFile", { path: "src/main.rs" }).build()();
```

JS developers expect:

```javascript
// Required
const content = fs.readFile("src/main.rs");
```

### 1.2 Solution

Two additions:

**OperationDescriptor** — a new optional method on `DatabaseDriver` that declares the driver's typed operation catalog. This is a framework-level feature. Any driver can opt in. The default returns an empty catalog, preserving backward compatibility with all existing drivers.

**V8 typed proxy codegen** — when `ctx.datasource("name")` is called and the driver declares an operation catalog, the V8 context builder generates a typed proxy object with methods matching the catalog. The generated methods construct `Query` structs internally and dispatch through the existing `execute()` contract. No new IPC verbs. No new host function bindings per operation.

**Filesystem driver** — the first consumer. A built-in driver exposing eleven filesystem operations through the typed proxy, with chroot enforcement and direct I/O in the worker process.

### 1.3 Collapse Principle

The driver owns the catalog. The V8 bridge builds the surface. Nobody maintains a parallel JS API file. Adding a new operation to any driver is a single Rust change — the JS surface regenerates automatically.

---

## 2. OperationDescriptor — Framework Addition

### 2.1 Types

```rust
/// Parameter type for JS-side validation before IPC dispatch.
#[derive(Clone, Debug)]
pub enum ParamType {
    String,
    Integer,
    Float,
    Boolean,
    /// Accepts string, number, boolean, array, or object
    Any,
}

/// Single parameter in an operation signature.
#[derive(Clone, Debug)]
pub struct Param {
    pub name: &'static str,
    pub param_type: ParamType,
    pub required: bool,
    pub default_value: Option<&'static str>,
}

impl Param {
    pub fn required(name: &'static str, param_type: ParamType) -> Self {
        Param { name, param_type, required: true, default_value: None }
    }

    pub fn optional(name: &'static str, param_type: ParamType, default: &'static str) -> Self {
        Param { name, param_type, required: false, default_value: Some(default) }
    }
}

/// Classifies an operation as read or write for DDL security alignment.
#[derive(Clone, Debug, PartialEq)]
pub enum OpKind {
    Read,
    Write,
}

/// Describes a single typed operation a driver exposes to handlers.
#[derive(Clone, Debug)]
pub struct OperationDescriptor {
    pub name: &'static str,
    pub kind: OpKind,
    pub params: &'static [Param],
    /// Brief description for documentation and error messages.
    pub description: &'static str,
}

impl OperationDescriptor {
    pub fn read(name: &'static str, params: &'static [Param], description: &'static str) -> Self {
        OperationDescriptor { name, kind: OpKind::Read, params, description }
    }

    pub fn write(name: &'static str, params: &'static [Param], description: &'static str) -> Self {
        OperationDescriptor { name, kind: OpKind::Write, params, description }
    }
}
```

### 2.2 DatabaseDriver Trait Amendment

```rust
#[async_trait]
pub trait DatabaseDriver: Send + Sync {
    fn name(&self) -> &str;

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError>;

    fn supports_transactions(&self) -> bool { false }
    fn supports_prepared_statements(&self) -> bool { false }

    /// Returns the typed operation catalog for V8 proxy codegen.
    /// Default: empty — driver uses standard Query/execute() dispatch.
    /// Override to declare typed methods available on ctx.datasource("name").
    fn operations(&self) -> &[OperationDescriptor] {
        &[]
    }
}
```

The default returns an empty slice. Existing drivers (PostgreSQL, MySQL, SQLite, Memcached, EventBus, Faker, rps-client) are unaffected. Drivers that want typed JS methods override `operations()`.

### 2.3 Backward Compatibility

When `operations()` returns an empty slice, `ctx.datasource("name")` returns the existing `DatasourceBuilder` (pseudo DataView builder). When `operations()` returns a non-empty slice, `ctx.datasource("name")` returns the typed proxy. The two are mutually exclusive per datasource — a driver either opts into the typed catalog or stays on the builder API.

A driver that implements both `operations()` and also works through DataViews (declared in TOML) continues to work through both paths. The typed proxy is for handler-driven programmatic access. DataViews declared in TOML bypass the proxy entirely and go through the standard DataView engine pipeline.

### 2.4 Future Consumers

Any driver can adopt the catalog incrementally:

| Driver | Candidate Operations |
|---|---|
| Redis | `get`, `set`, `del`, `hget`, `hset`, `hdel`, `lpush`, `rpop`, `expire` |
| MongoDB (plugin) | `findOne`, `find`, `insertOne`, `insertMany`, `updateOne`, `deleteOne` |
| Elasticsearch (plugin) | `search`, `index`, `get`, `delete`, `bulk` |

This spec does not define those catalogs. It defines the framework that makes them possible.

---

## 3. V8 Typed Proxy Codegen

### 3.1 When It Activates

During V8 context construction for a task, the context builder iterates the declared datasources in the `TaskContext`. For each datasource, it resolves the driver and calls `driver.operations()`. If the catalog is non-empty, a typed proxy object is generated and bound to the datasource name in the isolate context.

### 3.2 Generated Method Shape

For each `OperationDescriptor`, the codegen produces a JS method on the proxy object. The method:

1. Validates argument count against required/optional params
2. Validates argument types against declared `ParamType`
3. Constructs a `Query` struct with `operation = descriptor.name` and `parameters` mapped from positional args to named params
4. Dispatches through the existing host function binding for datasource operations
5. Returns the result to JS

Example: given this descriptor:

```rust
OperationDescriptor::read(
    "readFile",
    &[
        Param::required("path", ParamType::String),
        Param::optional("encoding", ParamType::String, "utf-8"),
    ],
    "Read file contents as string"
)
```

The codegen produces the equivalent of:

```javascript
proxy.readFile = function(path, encoding) {
    if (typeof path !== 'string') throw new TypeError("readFile: 'path' must be a string");
    encoding = encoding ?? "utf-8";
    if (typeof encoding !== 'string') throw new TypeError("readFile: 'encoding' must be a string");
    return __rivers_datasource_dispatch("project_files", {
        operation: "readFile",
        parameters: { path, encoding }
    });
};
```

`__rivers_datasource_dispatch` is the existing host function binding that resolves the datasource token and calls the driver. The typed proxy adds type checking and ergonomic method signatures on top of the same dispatch path.

### 3.3 ParamType Validation Rules

| ParamType | JS `typeof` check |
|---|---|
| `String` | `typeof arg === 'string'` |
| `Integer` | `typeof arg === 'number' && Number.isInteger(arg)` |
| `Float` | `typeof arg === 'number'` |
| `Boolean` | `typeof arg === 'boolean'` |
| `Any` | no check — any JS value accepted |

Validation happens in JS before the IPC call fires. Failed validation throws `TypeError` with the operation name, parameter name, and expected type. The developer sees the error in their JS stack trace.

### 3.4 Return Value

The host function binding returns the driver's response as a JS value. The codegen does not impose structure on the return — it passes through whatever the driver returns. For the filesystem driver, this means the Rust host function binding in the worker process returns the result directly (see §7). For IPC-based drivers, this means the deserialized `QueryResult` mapped to JS.

---

## 4. Filesystem Driver

### 4.1 Registration

Built-in driver registered in `DriverFactory::new()` alongside faker, sqlite, redis, postgres, mysql, memcached, eventbus, and rps-client.

```rust
// In DriverFactory::new()
factory.register_database_driver(Arc::new(FilesystemDriver));
```

Driver name: `"filesystem"`.

### 4.2 Driver Implementation

```rust
pub struct FilesystemDriver;

#[async_trait]
impl DatabaseDriver for FilesystemDriver {
    fn name(&self) -> &str { "filesystem" }

    async fn connect(
        &self,
        params: &ConnectionParams,
    ) -> Result<Box<dyn Connection>, DriverError> {
        let root = Self::resolve_root(&params.database)?;
        Ok(Box::new(FilesystemConnection { root }))
    }

    fn operations(&self) -> &[OperationDescriptor] {
        &FILESYSTEM_OPERATIONS
    }
}
```

The `connect()` method resolves and validates the root directory at startup. `FilesystemConnection` holds the canonical absolute root path. No pool is needed — the "connection" is just a validated root path.

### 4.3 No Credentials

The filesystem driver requires no credentials. Datasources using it declare `nopassword = true` in `resources.toml` and omit `credentials_source` from config. Same pattern as the faker driver.

### 4.4 Connection Pool

Pool size of 1 is sufficient. The `FilesystemConnection` holds no state beyond the root path. The pool exists only to satisfy the pool manager contract. No connection timeout, no health check interval (ping validates the root directory still exists).

---

## 5. Chroot Security Model

### 5.1 Root Resolution at Startup

When the datasource is configured, the driver resolves the `database` field to a canonical absolute path:

```rust
impl FilesystemDriver {
    fn resolve_root(database: &str) -> Result<PathBuf, DriverError> {
        let path = PathBuf::from(database);

        // Must be absolute
        if !path.is_absolute() {
            return Err(DriverError::Connection(
                format!("filesystem root must be absolute path, got: {}", database)
            ));
        }

        // Canonicalize — resolves symlinks and normalizes
        let canonical = std::fs::canonicalize(&path)
            .map_err(|e| DriverError::Connection(
                format!("filesystem root does not exist or is not accessible: {} — {}", database, e)
            ))?;

        // Must be a directory
        if !canonical.is_dir() {
            return Err(DriverError::Connection(
                format!("filesystem root is not a directory: {}", canonical.display())
            ));
        }

        Ok(canonical)
    }
}
```

The canonical path is resolved once at startup. All runtime path resolution is relative to this canonical root.

### 5.2 Runtime Path Validation

Every operation receives a relative path from the JS handler. Before any I/O:

```rust
fn resolve_path(&self, relative: &str) -> Result<PathBuf, DriverError> {
    // Normalize separators — handlers always use forward slashes
    let normalized = relative.replace('\\', "/");

    // Reject absolute paths (Unix: starts with /, Windows: starts with drive letter or \\)
    if normalized.starts_with('/')
        || normalized.starts_with('\\')
        || (normalized.len() >= 2 && normalized.as_bytes()[1] == b':')
    {
        return Err(DriverError::Query(
            "absolute paths not permitted — all paths relative to datasource root".into()
        ));
    }

    // Join with root
    let joined = self.root.join(&normalized);

    // Canonicalize to resolve any .. or . components
    // For operations on non-existent paths (writeFile, mkdir), canonicalize the parent
    let canonical = Self::canonicalize_for_op(&joined)?;

    // Verify the canonical path starts with the root
    if !canonical.starts_with(&self.root) {
        return Err(DriverError::Forbidden(
            "path escapes datasource root".into()
        ));
    }

    // Reject symlinks in any component of the resolved path
    Self::reject_symlinks(&canonical)?;

    Ok(canonical)
}
```

### 5.3 Symlink and Junction Rejection

Symlinks are rejected at every level. A symlink anywhere in the resolved path — including intermediate directories — causes the operation to fail with `DriverError::Forbidden`.

```rust
fn reject_symlinks(path: &Path) -> Result<(), DriverError> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if current.exists() && current.symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return Err(DriverError::Forbidden(
                format!("symlink detected in path: {}", current.display())
            ));
        }
    }
    Ok(())
}
```

On Windows, `is_symlink()` detects file and directory symlinks but **not** NTFS junctions (mount points). Junctions are handled by the canonicalize-and-check model — `std::fs::canonicalize` resolves junctions to their real target path, and the `starts_with(&self.root)` check in §5.2 catches any junction that points outside the root. The two mechanisms are complementary: explicit symlink rejection catches symlinks, canonicalization catches junctions.

### 5.4 TOCTOU Acknowledgment

The canonicalize → check → operate sequence has a theoretical TOCTOU (time-of-check-time-of-use) window. Between validation and the actual I/O call, an external process could create a symlink. Platform-specific mitigations exist (`openat2` on Linux 5.6+, `O_NOFOLLOW` on macOS) but none cover all path components uniformly across Linux, macOS, and Windows. Building a platform abstraction layer for marginal gain is rejected.

The canonicalize-and-check model is the portable implementation on all three platforms. The residual TOCTOU risk is mitigated by OS-level directory permissions — the datasource root directory should be owned by the `riversd` process user with appropriate permissions. The chroot is a structural boundary preventing handler JS code from addressing paths outside the root. It is not a security sandbox against a hostile process with write access to the same directory tree.

### 5.5 Path Encoding

All paths are UTF-8. Non-UTF-8 filenames are not supported and will produce an error. This is a deliberate simplification — JS strings are UTF-16 internally, and non-UTF-8 filenames are a portability nightmare.

---

## 6. Operation Catalog

### 6.1 Full Catalog

Eleven operations. Read/write classification follows the five-op contract alignment described in §1.

```rust
static FILESYSTEM_OPERATIONS: &[OperationDescriptor] = &[
    // ── Reads ──
    OperationDescriptor::read("readFile", &[
        Param::required("path", ParamType::String),
        Param::optional("encoding", ParamType::String, "utf-8"),
    ], "Read file contents — utf-8 returns string, base64 returns base64-encoded string"),

    OperationDescriptor::read("readDir", &[
        Param::required("path", ParamType::String),
    ], "List directory entries — returns filenames (not full paths)"),

    OperationDescriptor::read("stat", &[
        Param::required("path", ParamType::String),
    ], "File/directory metadata — size, mtime, atime, isFile, isDirectory, mode"),

    OperationDescriptor::read("exists", &[
        Param::required("path", ParamType::String),
    ], "Check if path exists — returns boolean"),

    OperationDescriptor::read("find", &[
        Param::required("pattern", ParamType::String),
        Param::optional("max_results", ParamType::Integer, "1000"),
    ], "Recursive glob/regex search — returns array of relative paths"),

    OperationDescriptor::read("grep", &[
        Param::required("pattern", ParamType::String),
        Param::optional("path", ParamType::String, "."),
        Param::optional("max_results", ParamType::Integer, "1000"),
    ], "Search file contents by regex — returns array of {file, line, content}"),

    // ── Writes ──
    OperationDescriptor::write("writeFile", &[
        Param::required("path", ParamType::String),
        Param::required("content", ParamType::String),
        Param::optional("encoding", ParamType::String, "utf-8"),
    ], "Write file — creates parent directories if needed, overwrites if exists"),

    OperationDescriptor::write("mkdir", &[
        Param::required("path", ParamType::String),
    ], "Create directory — recursive (creates parents), no error if exists"),

    OperationDescriptor::write("delete", &[
        Param::required("path", ParamType::String),
    ], "Delete file or directory — recursive for directories (rm -rf)"),

    OperationDescriptor::write("rename", &[
        Param::required("oldPath", ParamType::String),
        Param::required("newPath", ParamType::String),
    ], "Rename/move file or directory within root"),

    OperationDescriptor::write("copy", &[
        Param::required("src", ParamType::String),
        Param::required("dest", ParamType::String),
    ], "Copy file or directory within root"),
];
```

### 6.2 Return Types

Following Node.js `fs` module conventions:

| Operation | Returns |
|---|---|
| `readFile` | `string` — UTF-8 decoded content, or base64-encoded string for `encoding: "base64"` |
| `readDir` | `string[]` — filenames only (not full paths), same as Node `fs.readdirSync()` |
| `stat` | `{ size: number, mtime: string, atime: string, ctime: string, isFile: boolean, isDirectory: boolean, mode: number }` |
| `exists` | `boolean` |
| `find` | `{ results: string[], truncated: boolean }` — relative paths from root |
| `grep` | `{ results: { file: string, line: number, content: string }[], truncated: boolean }` |
| `writeFile` | `void` (undefined) |
| `mkdir` | `void` (undefined) |
| `delete` | `void` (undefined) |
| `rename` | `void` (undefined) |
| `copy` | `void` (undefined) |

### 6.3 readFile Encoding

Follows Node.js convention:

```javascript
// Default: UTF-8 string
const text = fs.readFile("config.json");
const text = fs.readFile("config.json", "utf-8");

// Binary: base64-encoded string
const b64 = fs.readFile("image.png", "base64");
```

Supported encodings: `"utf-8"` (default), `"base64"`. Any other value produces `DriverError::Query("unsupported encoding")`.

For `writeFile` with `encoding: "base64"`, the `content` parameter is expected to be a base64-encoded string which the driver decodes to bytes before writing.

### 6.4 readDir

Flat listing only. Returns filenames, not full paths. Does not include `.` or `..`. Entries are not sorted — order is filesystem-dependent, same as Node.

For recursive directory traversal, use `find("*")`. All returned paths use forward slashes on all platforms.

### 6.5 stat

Timestamps are ISO 8601 strings (UTC). `mode` is the Unix permission bits as an integer on Linux and macOS. On Windows, `mode` returns `0` — Windows ACLs do not map to Unix mode bits and faking it would be misleading.

### 6.6 find

`pattern` supports glob syntax (`*`, `**`, `?`, `[abc]`). The search is always rooted at the datasource root directory.

Results capped at `max_results` (default 1000). If more matches exist, `truncated` is `true`. Results are relative paths from the datasource root.

Hidden files (starting with `.`) are included in results. The `pattern` parameter matches against the full relative path, not just the filename.

### 6.7 grep

`pattern` is a regex (Rust `regex` crate syntax). The search scans files under `path` (default: datasource root). Binary files (detected by null byte in first 8192 bytes) are skipped.

Results capped at `max_results` (default 1000). If more matches exist, `truncated` is `true`. Each match includes the file path (relative), line number (1-indexed), and the full line content.

### 6.8 writeFile

Creates parent directories automatically (equivalent to `mkdir -p` on the parent). If the file exists, it is overwritten. There is no append mode — write the full content.

### 6.9 mkdir

Always recursive. No error if the directory already exists. Same as `mkdir -p`.

### 6.10 delete

Files are deleted directly. Directories are deleted recursively, including all contents. Same as `rm -rf`. No confirmation, no undo. If the path does not exist, no error is returned (idempotent).

### 6.11 rename

Both `oldPath` and `newPath` must resolve within the chroot. The chroot check applies to both paths independently. Cross-filesystem renames are not supported — both paths must be on the same filesystem (the standard `rename(2)` constraint).

### 6.12 copy

Both `src` and `dest` must resolve within the chroot. For files, this is a byte-level copy. For directories, this is a recursive copy of the entire tree.

---

## 7. Execution Path — Direct I/O

### 7.1 Why Not IPC

Database drivers use IPC because the host process holds shared resources — connection pools, credentials, transaction state. The worker process cannot hold a PostgreSQL connection; it must route through the host.

The filesystem driver has no shared resource. The "connection" is a validated root path. The worker process can read and write the filesystem directly. Routing a 2MB file read through IPC — serialize in host, transmit over pipe, deserialize in worker — is pure overhead.

### 7.2 Architecture

```
Host Process (startup)                Worker Process (runtime)
──────────────────────                ─────────────────────────
FilesystemDriver registers            Receives resolved root path
  in DriverFactory                      as part of capability set

Datasource config validates           Rust host function bindings:
  root exists, is directory,            resolve_path() — chroot enforcement
  is canonical, no symlinks             readFile() — std::fs::read
                                        writeFile() — std::fs::write
                                        etc.

TaskContext includes:                 V8 typed proxy calls host functions
  datasource "files" →                  directly — no IPC round trip
  root: "/var/rivers/workspaces/x"
```

### 7.3 Capability Token

The filesystem datasource token in `TaskContext` carries the resolved canonical root path. Unlike database datasource tokens (which resolve to pooled connections on the host side), the filesystem token is self-contained — the worker has everything it needs to perform I/O.

```rust
pub enum DatasourceToken {
    /// Standard token — resolves to host-side connection pool
    Pooled { pool_id: String },
    /// Direct token — worker performs I/O locally
    Direct { driver: String, root: PathBuf },
}
```

When the V8 context builder sees a `Direct` token, it binds host function callbacks that perform I/O in the worker process using the `root` path. No IPC channel to the host is opened for this datasource.

### 7.4 Security Boundary

The chroot enforcement (§5) runs entirely in the worker process. The host process validated the root at startup. The worker process validates every individual path at runtime. This is the same two-gate model used elsewhere in Rivers — startup validation + runtime enforcement.

The worker process runs under the same OS user as the host process. File permissions are the OS-level boundary. Rivers does not attempt to provide finer-grained file permissions — the app developer owns the directory, owns the consequences.

---

## 8. Configuration Reference

### 8.1 Datasource Configuration

```toml
[data.datasources.project_files]
driver   = "filesystem"
database = "/var/rivers/workspaces/project-alpha"
```

| Field | Required | Description |
|---|---|---|
| `driver` | yes | Must be `"filesystem"` |
| `database` | yes | Absolute path to root directory — becomes the chroot boundary |

No `host`, `port`, `username`, or `credentials_source`. No `connection_pool` block — pool size is fixed at 1.

### 8.2 Resources Declaration

```toml
# resources.toml
[[resources]]
name       = "project_files"
x-type     = "filesystem"
nopassword = true
```

### 8.3 View Declaration

The view declares the filesystem datasource as a capability dependency, same as any other datasource:

```toml
[api.views.file_manager]
path       = "/api/files/{path}"
view_type  = "Rest"
datasources = ["project_files"]

methods.GET.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/files.ts",
    entrypoint_function = "onRead",
    resources           = ["project_files"]
}}

methods.POST.handler = { CodeComponent = {
    language            = "typescript",
    module              = "handlers/files.ts",
    entrypoint_function = "onWrite",
    resources           = ["project_files"]
}}
```

### 8.4 Extra Configuration

Optional driver-specific configuration via the `extra` table:

```toml
[data.datasources.project_files.extra]
# Maximum file size for readFile/writeFile operations (bytes, default: 50MB)
max_file_size = 52428800
# Maximum depth for recursive operations: find, grep, delete, copy (default: 100)
max_depth = 100
```

---

## 9. JS Handler API

### 9.1 Typed Proxy

```typescript
export function onRead(ctx: any): void {
    const fs = ctx.datasource("project_files");

    // Read file (UTF-8)
    const content = fs.readFile("src/main.rs");

    // Read file (base64 for binary)
    const imageData = fs.readFile("assets/logo.png", "base64");

    // List directory
    const entries = fs.readDir("src/");
    // entries: ["main.rs", "lib.rs", "utils/"]

    // Check existence
    if (fs.exists("config.json")) {
        const config = JSON.parse(fs.readFile("config.json"));
    }

    // File metadata
    const info = fs.stat("src/main.rs");
    // info: { size: 4096, mtime: "2026-04-16T...", isFile: true, isDirectory: false, mode: 33188 }

    // Search by filename pattern
    const rustFiles = fs.find("**/*.rs");
    // rustFiles: { results: ["src/main.rs", "src/lib.rs"], truncated: false }

    // Search file contents
    const todos = fs.grep("TODO|FIXME", "src/");
    // todos: { results: [{ file: "src/main.rs", line: 42, content: "// TODO: fix this" }], truncated: false }

    ctx.resdata = { content };
}

export function onWrite(ctx: any): void {
    const fs = ctx.datasource("project_files");

    // Write file (creates parent dirs)
    fs.writeFile("output/report.json", JSON.stringify(ctx.request.body));

    // Write binary from base64
    fs.writeFile("output/image.png", base64Data, "base64");

    // Create directory
    fs.mkdir("output/reports/2026");

    // Rename
    fs.rename("output/draft.json", "output/final.json");

    // Copy
    fs.copy("templates/default.json", "output/config.json");

    // Delete
    fs.delete("output/temp/");

    ctx.resdata = { status: "ok" };
}
```

### 9.2 Error Handling

Filesystem operations throw on error. The handler can catch and handle:

```typescript
export function onSafeRead(ctx: any): void {
    const fs = ctx.datasource("project_files");
    try {
        ctx.resdata = { content: fs.readFile(ctx.request.params.path) };
    } catch (e) {
        if (e.message.includes("not found")) {
            ctx.resdata = { error: "file not found" };
            ctx.status = 404;
        } else if (e.message.includes("escapes datasource root")) {
            ctx.resdata = { error: "access denied" };
            ctx.status = 403;
        } else {
            throw e;
        }
    }
}
```

---

## 10. Error Model

The filesystem driver maps OS errors to `DriverError` variants:

| Condition | DriverError Variant | Message Pattern |
|---|---|---|
| File/directory not found | `Query` | `"not found: {path}"` |
| Permission denied (OS-level) | `Query` | `"permission denied: {path}"` |
| Path escapes chroot | `Forbidden` | `"path escapes datasource root"` |
| Symlink detected | `Forbidden` | `"symlink detected in path: {component}"` |
| Absolute path provided | `Query` | `"absolute paths not permitted"` |
| File too large | `Query` | `"file exceeds max_file_size: {size} bytes"` |
| Invalid encoding | `Query` | `"unsupported encoding: {enc}"` |
| Directory not empty (for ops that expect file) | `Query` | `"path is a directory, expected file: {path}"` |
| Not a directory (for ops that expect dir) | `Query` | `"path is not a directory: {path}"` |
| Disk full / I/O error | `Internal` | `"I/O error: {os_error}"` |
| Root directory gone at runtime | `Connection` | `"datasource root no longer exists: {root}"` |

Security-related errors (`Forbidden`) do not include the resolved canonical path in the message — only the fact that the violation occurred. This prevents information leakage about the host filesystem layout.

---

## 11. Admin Operations

```rust
fn admin_operations(&self) -> &[&str] {
    &[]
}
```

Empty. No filesystem operations are gated behind `ddl_execute()`. The app developer declared the datasource, declared the view dependency, owns the directory. If they delete their files, they're gone.

---

## 12. Implementation Notes

### 12.1 Crate Location

The filesystem driver is built-in: `crates/rivers-drivers-builtin/src/filesystem.rs`. The `OperationDescriptor` types are added to `crates/rivers-driver-sdk/src/lib.rs`.

### 12.2 Dependencies

No additional crate dependencies. Uses `std::fs` and `std::path` for all I/O. The `glob` crate (already in the workspace for other uses) can be used for `find` pattern matching. The `regex` crate (already in the workspace) is used for `grep`.

### 12.3 Cross-Platform Considerations

The filesystem driver targets Linux, macOS, and Windows.

**Path separators.** The driver accepts forward slashes (`/`) in all paths from JS handlers on all platforms. On Windows, `std::path::PathBuf::join()` handles the translation. Handlers never use backslashes — the JS API surface is normalized to forward slashes regardless of OS. Return values from `readDir`, `find`, and `grep` also use forward slashes on all platforms.

**Symlink detection.** `symlink_metadata().file_type().is_symlink()` works on all three platforms for symlinks. On Windows, NTFS junctions are **not** detected by `is_symlink()` — they are caught instead by the canonicalize-and-check model, since `std::fs::canonicalize` resolves junctions to their real target and the `starts_with` check rejects paths outside the root.

**stat `mode` field.** Returns Unix permission bits on Linux and macOS. On Windows, returns `0` — Windows ACLs do not map to Unix mode bits. The spec documents this in §6.5.

**rename atomicity.** On Linux and macOS, `rename(2)` is atomic within the same filesystem. On Windows, `std::fs::rename` uses `MoveFileExW` which is atomic for files on the same volume but not for directories. This is a known Windows limitation and is not mitigated.

**File locking.** The driver does not implement file locking. Concurrent writes to the same file from multiple handlers produce last-write-wins behavior on all platforms. This is the same contract as Node.js `fs.writeFileSync`.

**Max path length.** Windows has a 260-character path limit by default (`MAX_PATH`). The driver does not attempt to work around this with `\\?\` prefix paths. Deeply nested paths on Windows may fail with an OS-level error, surfaced as `DriverError::Query`.

### 12.4 Testing

The filesystem driver is testable without external infrastructure — just a temporary directory. Tests should cover:

- Chroot enforcement: `../` traversal, absolute paths, symlink injection
- All eleven operations: happy path + error paths
- Encoding toggle: UTF-8 and base64 for readFile/writeFile
- Truncation: find/grep with more results than max_results
- Edge cases: empty files, empty directories, deeply nested paths, unicode filenames
- Concurrent access: multiple handlers writing to the same directory
- Path separator normalization: forward slashes on Windows
- Platform-specific: stat mode on Windows returns 0, symlink/junction rejection on Windows

### 12.5 Canary Fleet

Add a `canary-filesystem` app to the canary fleet with tests covering:

- Basic CRUD: write → read → stat → delete
- Chroot escape attempts
- find/grep with bounded results
- Binary file round-trip via base64

### 12.5 Feature Inventory Update

Add to `rivers-feature-inventory.md` §6.1 Built-in Drivers:

```
- **Filesystem** (std::fs): chroot-sandboxed directory access, eleven typed operations,
  direct I/O in worker process, no credentials required
```

Add to §6.6:

```
- `OperationDescriptor` — driver-declared typed operation catalog for V8 proxy codegen.
  Drivers that declare operations get typed JS methods on `ctx.datasource("name")`
  instead of the pseudo DataView builder. Framework-level feature — any driver can opt in.
```

---

## Revision History

| Version | Date | Changes |
|---|---|---|
| 1.0 | 2026-04-16 | Initial specification — OperationDescriptor framework + filesystem driver |
