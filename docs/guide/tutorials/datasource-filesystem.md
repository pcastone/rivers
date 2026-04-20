# Tutorial: Filesystem Datasource

**Rivers v0.54.0**

## Overview

The filesystem driver exposes a chroot-sandboxed directory as a datasource with eleven typed operations. Unlike SQL or HTTP drivers, it has no connection pool — operations run directly in the V8 worker thread against a single resolved root path. No credentials, no network, no IPC round trip.

Use the filesystem driver when:

- A handler needs to read application-managed files (uploaded attachments, generated reports, static content)
- You want a simple persistence layer for short-lived state (scratch scratch files, small caches) without standing up a database
- You're building tooling that inspects a specific directory (config watcher, log tailer)

The filesystem driver is a built-in driver. No plugin dylib is required.

## Prerequisites

- A Rivers app bundle with a valid `manifest.toml`
- An absolute path on disk that already exists and is a directory (the driver canonicalizes the path at connect time and rejects anything that isn't)

## Typed proxy surface

Unlike pooled drivers that use the `ctx.datasource(name).fromQuery(sql).build()` chain, the filesystem driver opts into a **typed proxy**. Calling `ctx.datasource("fs")` returns an object whose methods are generated from the driver's operation catalog. Arguments are type-checked in JavaScript *before* dispatch; results are auto-unwrapped to natural JS shapes (scalars → strings, single row → object, multiple rows → array).

```javascript
const fs = ctx.datasource("fs");
const body = fs.readFile("config.json"); // → string
const entries = fs.readDir(".");         // → [{name: "a.txt"}, {name: "b.txt"}]
```

## Step 1: Declare the datasource

In your app's `resources.toml`:

```toml
[[datasources]]
name       = "fs"
driver     = "filesystem"
x-type     = "filesystem"
nopassword = true
required   = true
```

`nopassword = true` — filesystem access needs no credentials.

## Step 2: Configure the root path

In your app's `app.toml`:

```toml
[data.datasources.fs]
name       = "fs"
driver     = "filesystem"
database   = "/var/rivers/uploads"
nopassword = true
```

The `database` field is the resource root. It **must**:

- be an absolute path
- already exist when `riversd` starts
- be a directory (not a file, not a symlink to a file)

On startup the driver calls `std::fs::canonicalize` on this path and stores the result. Every subsequent operation is resolved against the canonical root.

## Step 3: Use the driver from a handler

```toml
[api.views.read_config]
path      = "/config"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.read_config.handler]
type       = "codecomponent"
language   = "typescript"
module     = "libraries/handlers/config.ts"
entrypoint = "readConfig"
```

```typescript
// libraries/handlers/config.ts
export function readConfig(ctx: any): void {
    const body = ctx.datasource("fs").readFile("config.json");
    ctx.resdata = JSON.parse(body);
}
```

## Eleven operations

All paths are relative to the resource root. Absolute paths are rejected with a `Query` error.

### Reads

```javascript
fs.readFile("path.txt");                     // → string (utf-8)
fs.readFile("photo.jpg", "base64");          // → base64 string
fs.readDir(".");                             // → [{name}]
fs.stat("file.txt");                         // → {size, mtime, atime, ctime, isFile, isDirectory, mode}
fs.exists("file.txt");                       // → boolean; chroot escape returns false, not error
fs.find("*.md");                             // → {results: [path, …], truncated: bool}
fs.find("**/*.ts", 500);                     // max_results=500
fs.grep("TODO", ".");                        // → {results: [{file, line, content}], truncated: bool}
```

`find` uses glob patterns and returns paths relative to the root. `grep` uses a full regex (via the `regex` crate) and skips files with a `\0` byte in the first 8 KB.

### Writes

```javascript
fs.writeFile("out.txt", "hello");            // utf-8, overwrites, creates parent dirs
fs.writeFile("bin.dat", "AQID", "base64");   // decode before write
fs.mkdir("a/b/c");                           // recursive, idempotent
fs.delete("old.txt");                        // file or recursive dir; missing → no-op
fs.rename("old.txt", "new.txt");             // must stay within root
fs.copy("src.txt", "dst.txt");               // file or recursive dir
```

Optional `encoding` on `readFile`/`writeFile` is `"utf-8"` (default) or `"base64"`. Any other value throws.

## Chroot model

Two layers enforce the sandbox:

1. **At configure time**: the root is canonicalized and stored. Symlinks in the configured path are resolved once, before any handler runs.
2. **At operation time**: every relative path is resolved against the canonical root, and any intermediate symlink — even one pointing back inside the root — is rejected with `Forbidden`.

```javascript
fs.readFile("../../etc/passwd");  // throws — Query: "path escapes datasource root"
fs.readFile("/etc/passwd");       // throws — Query: "absolute paths not permitted"
fs.exists("../../etc/passwd");    // returns false (not an error)
```

`exists` is the one exception: a traversal that escapes the root is treated as "not visible" rather than an error, which matches common JS intuition.

## Limits

Two safety knobs apply per-operation:

- **`max_file_size`** — default 50 MB. Any `readFile` / `writeFile` that exceeds this byte count is rejected with a `Query` error before the I/O runs.
- **`max_depth`** — default 100. Applies to `grep`'s recursive walk. Directories beyond this depth are silently skipped.

Both are set on the `FilesystemConnection` at connection time. In v0.54 they use the defaults above; richer `extra` config plumbing is planned.

## Error model

| JS error | Rust variant | Meaning |
|----------|--------------|---------|
| `Error: not found: …` | `DriverError::Query` | File or directory absent |
| `Error: permission denied: …` | `DriverError::Query` | Unix permission bits rejected the op |
| `Error: absolute paths not permitted` | `DriverError::Query` | Handler passed an absolute path |
| `Error: path escapes datasource root` | `DriverError::Forbidden` | Traversal landed outside the root |
| `Error: symlink detected in path` | `DriverError::Forbidden` | Intermediate symlink rejected |
| `Error: file exceeds max_file_size` | `DriverError::Query` | Size limit tripped |
| `TypeError: … must be a string` | (not dispatched) | Typed-proxy arg check rejected before call |

Argument errors surface as `TypeError` because the typed proxy validates before entering the host. `Error` covers runtime failures in the driver itself.

## Example: upload-then-process

```typescript
// libraries/handlers/upload.ts
export function handleUpload(ctx: any): void {
    const fs = ctx.datasource("fs");
    const body: string = ctx.request.body_base64;
    const filename = "uploads/" + Rivers.crypto.randomHex(8) + ".bin";

    fs.writeFile(filename, body, "base64");

    const info = fs.stat(filename);
    ctx.resdata = {
        stored_as: filename,
        size_bytes: info.size,
        uploaded_at: info.mtime
    };
}
```

## Testing locally

A throwaway bundle works for experiments:

```bash
mkdir -p /tmp/rivers-fs-scratch
riverpackage init scratch --driver filesystem
# edit scratch/app.toml: database = "/tmp/rivers-fs-scratch"
cargo deploy /tmp/scratch-dist
/tmp/scratch-dist/bin/riversctl start --foreground
```

## See also

- Spec: `docs/arch/rivers-filesystem-driver-spec.md`
- Feature inventory §6.1 and §6.6: `docs/arch/rivers-feature-inventory.md`
- Canary profile (live reference): `canary-bundle/canary-filesystem/`
