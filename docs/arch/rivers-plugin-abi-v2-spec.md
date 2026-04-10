# Rivers Plugin ABI v2 — Synchronous Driver Interface

Date: 2026-04-09
Status: Draft

## Problem

cdylib driver plugins crash the host process (SIGABRT) because they statically link their own tokio runtime. When the host calls `driver.connect()` through the async trait, the plugin's internal tokio panics ("no reactor running"). The panic crosses the FFI boundary as a "foreign exception" that Rust cannot catch.

The current Plugin ABI v1 uses Rust `async_trait` across the dylib boundary, which requires both sides to share the same tokio runtime — a fragile assumption that breaks with cdylib compilation.

## Root Cause Analysis

```
Current flow (broken):

  Host (riversd)                    cdylib Plugin (mongodb)
  ─────────────                     ──────────────────────
  tokio runtime A                   tokio runtime B (static, no reactor)
       │                                   │
  DataViewExecutor                         │
       │                                   │
  DriverFactory::connect()                 │
       │                                   │
  driver.connect(params).await ──────►  MongoDriver::connect()
       │                                   │
       │                            mongodb::Client::with_options()
       │                                   │
       │                            tokio::runtime::Handle::current()
       │                                   │
       │                            PANIC: "no reactor running"
       │                                   │
       │                            panic crosses FFI → abort()
       ▼
  SIGABRT (exit 134)
```

The fundamental issue: **async Rust traits cannot safely cross cdylib FFI boundaries** because each compilation unit has its own tokio thread-locals.

## Solution: Synchronous C-ABI for Plugin Drivers

Follow the same pattern already proven by the Engine SDK (`rivers-engine-sdk`):

- Engines export synchronous C-ABI functions (`_rivers_engine_execute`)
- Input/output is JSON over raw byte buffers
- The host manages all async dispatch
- No tokio dependency in the engine

Apply this pattern to driver plugins:

```
Proposed flow (v2):

  Host (riversd)                    cdylib Plugin (mongodb)
  ─────────────                     ──────────────────────
  tokio runtime                     NO tokio dependency
       │                                   │
  DriverFactory::connect()                 │
       │                                   │
  spawn_blocking ─────────────────►  _rivers_driver_connect()
       │                            (synchronous C-ABI function)
       │                                   │
       │                            returns opaque handle + JSON result
       │                                   │
  DriverFactory::execute()                 │
       │                                   │
  spawn_blocking ─────────────────►  _rivers_driver_execute()
       │                            (synchronous C-ABI, takes handle)
       │                                   │
       │                            returns JSON result bytes
       ▼
  Result<QueryResult, DriverError>
```

## Plugin ABI v2 — C Symbol Contract

Each driver plugin cdylib exports these symbols:

```c
// ABI version check (same as v1)
uint32_t _rivers_abi_version(void);

// Driver registration (same as v1)
void _rivers_register_driver(DriverRegistrar* registrar);

// NEW: Synchronous connect — returns opaque connection handle
// Input: JSON {"host":"...", "port":N, "database":"...", "username":"...", "options":{...}}
// Output: JSON {"ok":true, "handle":N} or {"error":"..."}
int32_t _rivers_driver_connect(
    const char* driver_name,
    const uint8_t* params_json, size_t params_len,
    uint8_t** out_ptr, size_t* out_len
);

// NEW: Synchronous execute — runs query on a connection handle
// Input: JSON {"handle":N, "query":"...", "params":{...}}
// Output: JSON {"rows":[...], "affected_rows":N} or {"error":"..."}
int32_t _rivers_driver_execute(
    uint64_t handle,
    const uint8_t* query_json, size_t query_len,
    uint8_t** out_ptr, size_t* out_len
);

// NEW: Synchronous DDL execute
int32_t _rivers_driver_ddl_execute(
    uint64_t handle,
    const uint8_t* query_json, size_t query_len,
    uint8_t** out_ptr, size_t* out_len
);

// NEW: Close connection handle
void _rivers_driver_close(uint64_t handle);

// Buffer management (same pattern as engine SDK)
void _rivers_driver_free(uint8_t* ptr, size_t len);
```

## Plugin-Side Implementation

Each plugin manages its own connection state internally. The plugin CAN use tokio internally if it creates its own runtime during `_rivers_driver_connect` — the key difference is that the host never calls `.await` on plugin code.

```rust
// Inside the cdylib plugin
use std::collections::HashMap;
use std::sync::{atomic::AtomicU64, Mutex};

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
static CONNECTIONS: Mutex<HashMap<u64, mongodb::Client>> = Mutex::new(HashMap::new());

#[no_mangle]
pub extern "C" fn _rivers_driver_connect(
    driver_name: *const u8,
    params_json: *const u8, params_len: usize,
    out_ptr: *mut *mut u8, out_len: *mut usize,
) -> i32 {
    // Plugin creates its own tokio runtime for the connect call
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = rt.block_on(async {
        mongodb::Client::with_uri_str(&uri).await
    });
    // Store connection, return handle
    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    CONNECTIONS.lock().unwrap().insert(handle, client);
    // Write {"ok":true, "handle": handle} to output
    // ...
    0
}
```

The critical difference: **the plugin owns its runtime lifetime**. It creates `Runtime::new()` during connect, and the runtime lives as long as the connection handle. The host never needs to provide a reactor.

## Host-Side Implementation

### DriverFactory changes

`DriverFactory` gets a new internal enum:

```rust
enum DriverImpl {
    /// Built-in or statically-linked driver (async trait)
    Native(Arc<dyn DatabaseDriver>),
    /// cdylib plugin driver (synchronous C-ABI)
    Plugin {
        lib: Arc<libloading::Library>,
        connect_fn: ConnectFn,
        execute_fn: ExecuteFn,
        ddl_execute_fn: DdlExecuteFn,
        close_fn: CloseFn,
        free_fn: FreeFn,
    },
}
```

`DriverFactory::connect()` dispatches to the appropriate implementation:

```rust
pub async fn connect(&self, name: &str, params: &ConnectionParams) -> Result<Box<dyn Connection>, DriverError> {
    match self.get_impl(name)? {
        DriverImpl::Native(driver) => driver.connect(params).await,
        DriverImpl::Plugin { .. } => {
            // Serialize params to JSON, call _rivers_driver_connect via spawn_blocking
            // Return a PluginConnection that wraps the handle
        }
    }
}
```

### PluginConnection

A `Connection` impl that wraps the C-ABI handle:

```rust
struct PluginConnection {
    handle: u64,
    execute_fn: ExecuteFn,
    ddl_execute_fn: DdlExecuteFn,
    close_fn: CloseFn,
    free_fn: FreeFn,
}

#[async_trait]
impl Connection for PluginConnection {
    async fn execute(&mut self, query: &Query) -> Result<QueryResult, DriverError> {
        // Serialize query to JSON
        // Call _rivers_driver_execute via spawn_blocking
        // Deserialize result
    }
}

impl Drop for PluginConnection {
    fn drop(&mut self) {
        (self.close_fn)(self.handle);
    }
}
```

## Migration Path

### Phase 1: Add v2 symbols to plugins (backward compatible)
- Add `_rivers_driver_connect`, `_rivers_driver_execute`, etc. to each plugin
- Keep existing `_rivers_register_driver` for v1 compatibility
- Host prefers v2 symbols if present, falls back to v1

### Phase 2: Update DriverFactory to use v2
- `load_plugins()` checks for v2 symbols first
- If found, register as `DriverImpl::Plugin`
- If not found, register as `DriverImpl::Native` (v1 path)

### Phase 3: Remove v1 async trait from plugins
- Plugins only export C-ABI symbols
- Remove `#[async_trait]` from plugin implementations
- Remove tokio dependency from plugin Cargo.toml (plugins that need it can add their own)

## What This Fixes

| Issue | v1 (current) | v2 (proposed) |
|---|---|---|
| cdylib tokio crash | SIGABRT | Impossible — no shared async |
| Plugin panics | Process abort | catch_unwind at C-ABI boundary |
| Shared libstd requirement | Required for dynamic linking | Not required — pure C-ABI |
| Plugin language | Rust only | Any language (C-ABI) |
| Build complexity | -C prefer-dynamic + rpath fixup | Standard cdylib build |

## What Doesn't Change

- Built-in drivers (sqlite, postgres, mysql, redis, faker) — continue using Native async trait
- Engine SDK — already uses synchronous C-ABI (unchanged)
- DataViewExecutor — calls DriverFactory which handles the dispatch
- Bundle loader, host callbacks — unchanged (they call DriverFactory)

## ABI Version

Bump plugin ABI version from 1 to 2. The loader checks `_rivers_abi_version()`:
- Version 1: use `_rivers_register_driver` (async trait path)
- Version 2: use `_rivers_driver_connect` / `_rivers_driver_execute` (sync C-ABI path)

Plugins can export both for backward compatibility during migration.

## Comparison with Engine SDK

| Aspect | Engine SDK (existing) | Plugin ABI v2 (proposed) |
|---|---|---|
| Boundary | `_rivers_engine_execute(json) → json` | `_rivers_driver_connect/execute(json) → json` |
| State | Stateless (context per call) | Stateful (connection handle) |
| Async | Host manages | Host manages via spawn_blocking |
| Serialization | JSON over byte buffers | JSON over byte buffers |
| Buffer management | `_rivers_engine_free` | `_rivers_driver_free` |

The patterns are nearly identical. The only difference is that drivers have persistent connection state (handles) while engines are stateless.
