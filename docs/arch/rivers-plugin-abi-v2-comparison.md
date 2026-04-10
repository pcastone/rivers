# Plugin ABI v2 — Comparison with Rust Plugin Ecosystems

Date: 2026-04-09

## How Other Rust Projects Handle Async Plugins

### 1. Synchronous C-ABI with Host-Managed Async

**How it works:** Plugins export `extern "C"` functions. All I/O is synchronous from the plugin's perspective. The host wraps calls in `spawn_blocking` or dedicated threads.

**Who uses it:**
- **Nginx modules** — all module callbacks are synchronous C functions
- **PostgreSQL extensions** — synchronous C-ABI, database handles I/O scheduling
- **Rivers Engine SDK** — `_rivers_engine_execute` is synchronous C-ABI, host dispatches via spawn_blocking
- **Most libloading-based Rust plugin systems** — the standard pattern for cdylib plugins

**Trade-offs:**
- Pro: No tokio dependency in plugins. No runtime conflicts. Works with any language.
- Pro: `catch_unwind` works cleanly at C-ABI boundary
- Con: Plugin can't do concurrent I/O internally (but can create its own runtime)
- Con: Serialization overhead (JSON over buffers)

**Fit for Rivers:** Excellent — this is exactly what the Engine SDK already does. Proven in the codebase.

### 2. Shared dylib for Runtime (what we just built)

**How it works:** Compile the runtime (tokio, libstd) as a shared library. Both host and plugins link against it dynamically, sharing thread-locals.

**Who uses it:**
- **Python's C extension model** — all extensions share libpython
- **Node.js native addons** — share libuv/V8 via dynamic linking
- **Rivers v0.53.7** — our `-C prefer-dynamic` fix

**Trade-offs:**
- Pro: Plugins can use async natively — no serialization overhead
- Pro: Minimal code changes to existing plugins
- Con: Fragile — requires exact version matching of shared libs
- Con: Complex rpath management (different per platform)
- Con: LTO disabled for entire build
- Con: Larger deployment (must ship libstd, librivers_runtime)
- Con: Still Rust-only (can't write plugins in C/Go/Python)

**Fit for Rivers:** Works as a stopgap. We proved it prevents the SIGABRT. But brittle for production — version skew between host and plugin shared libs causes subtle bugs.

### 3. WASM Sandboxing (Wasmtime/Extism approach)

**How it works:** Plugins compile to WASM. The host runs them in a sandboxed WASM runtime (wasmtime). Communication is via WASI or custom host functions. Complete isolation.

**Who uses it:**
- **Wasmtime** — the canonical Rust WASM runtime
- **Extism** — plugin framework built on wasmtime, supports Rust/Go/C/Python/JS plugins
- **Envoy Proxy** — WASM filters for HTTP middleware
- **Shopify Functions** — WASM plugins for commerce logic
- **Fermyon Spin** — WASM microservice framework

**Trade-offs:**
- Pro: Complete isolation — plugin crashes can't affect host
- Pro: Language-agnostic — any language that compiles to WASM
- Pro: Sandboxed — plugins can't access host filesystem/network without explicit grants
- Pro: No shared library or runtime concerns
- Con: Performance overhead (WASM interpreter/JIT vs native)
- Con: Database drivers (mongodb, postgres) may not compile to WASM easily
- Con: Significant rearchitecture
- Con: WASI networking is still maturing

**Fit for Rivers:** Future direction, but too large a change for the immediate fix. Rivers already has wasmtime for CodeComponent handlers — could extend to drivers long-term.

### 4. Process Isolation

**How it works:** Each plugin runs in a child process. Communication via IPC (Unix sockets, stdin/stdout, gRPC). Plugin crashes don't affect the host.

**Who uses it:**
- **HashiCorp go-plugin** — Go plugin system used by Terraform, Vault, Nomad
- **VS Code extensions** — each extension runs in a separate Node.js process
- **Chrome extensions** — separate renderer process per extension
- **Neovim plugins** — msgpack-rpc over stdio

**Trade-offs:**
- Pro: Complete crash isolation — plugin crash = restart child process
- Pro: Language-agnostic — any language that speaks the IPC protocol
- Pro: No shared library concerns
- Con: IPC overhead (serialization, context switching, latency)
- Con: More complex lifecycle management (spawn, health check, restart)
- Con: Higher resource usage (one process per plugin)
- Con: Connection state harder to manage across process boundary

**Fit for Rivers:** Overkill for driver plugins. The IPC latency on every query would be significant. Better suited for long-running background workers, not per-request database queries.

### 5. abi_stable Crate

**How it works:** Provides stable ABI types (`RVec`, `RString`, `RBox`) that work across different Rust compiler versions. Async support via `#[sabi_trait]` with a custom future type.

**Who uses it:**
- **abi_stable** ecosystem — a few Rust plugin projects
- Limited adoption due to complexity

**Trade-offs:**
- Pro: Rust-native — plugins are Rust libraries with stable ABI
- Pro: Supports async (via custom future types)
- Pro: No serialization overhead — direct memory sharing
- Con: Heavy dependency — wraps every type in ABI-stable wrappers
- Con: Still doesn't solve the tokio thread-local problem cleanly
- Con: Limited ecosystem adoption — few examples/documentation
- Con: Compiler version coupling (less strict than raw cdylib but still present)

**Fit for Rivers:** Too complex for the problem. The C-ABI JSON approach is simpler and already proven.

## Recommendation Matrix

| Approach | Crash Safety | Performance | Complexity | Language-Agnostic | Rivers Fit |
|---|---|---|---|---|---|
| **Sync C-ABI (v2 proposed)** | Excellent | Good (JSON overhead) | Low | Yes | **Best** |
| Shared dylib (current fix) | Good | Best (native) | Medium (rpath) | No | Stopgap |
| WASM sandbox | Excellent | Medium | High | Yes | Future |
| Process isolation | Excellent | Low (IPC) | High | Yes | Overkill |
| abi_stable | Good | Good | Very High | No | Over-engineered |

### 6. Tauri Plugin System — Shared Tokio Singleton

**How it works:** Tauri owns a single `tokio::Runtime`. All plugins run as async commands via `async_runtime::spawn`. Plugins use `tauri::State<T>` for shared channels.

**Trade-offs:**
- Pro: Simple — one runtime for everything, plugins can use tokio directly
- Con: Tight coupling to tokio version (all plugins must match)
- Con: Single runtime for all plugins (contention)

**Fit for Rivers:** Different architecture (desktop app vs server). Rivers separates engines from drivers; Tauri doesn't.

### 7. async_ffi Crate — FFI-Safe Futures

**How it works:** Provides `FfiFuture<T>` — a `repr(C)` wrapper for Rust futures that works across cdylib boundaries. Can be combined with `abi_stable`.

**Trade-offs:**
- Pro: Async works across FFI without shared runtime
- Con: Extra allocation per future conversion
- Con: Panic handling via `catch_unwind` (aborts on cleanup panics)
- Con: Rust-only, not language-agnostic

**Fit for Rivers:** Possible but adds complexity. The sync C-ABI approach is simpler and already proven.

## Ecosystem Research Sources

- [Tokio Issue #1964](https://github.com/tokio-rs/tokio/issues/1964) — "Failed to spawn tasks in dynamic library"
- [Tokio Issue #6927](https://github.com/tokio-rs/tokio/issues/6927) — "Support for dynamic linking"
- [Plugins in Rust: Reducing Pain with Dependencies](https://nullderef.com/blog/plugin-abi-stable/) — abi_stable approach
- [async_ffi Crate](https://docs.rs/async-ffi/latest/async_ffi/) — FFI-safe futures
- [Wasmtime Async Host Functions](https://docs.wasmtime.dev/examples-async.html)
- [How to Build a Plugin System in Rust (Arroyo)](https://www.arroyo.dev/blog/rust-plugin-systems/)
- [Rust FFI Guide: Dynamic Loading](https://s3.amazonaws.com/temp.michaelfbryan.com/dynamic-loading/index.html)
- [Extism Async Discussion #525](https://github.com/extism/extism/discussions/525)
- [Tauri Async Runtime](https://docs.rs/tauri/latest/tauri/async_runtime/index.html)
- [Tokio Bridging with Sync Code](https://tokio.rs/tokio/topics/bridging)

## Conclusion

**The synchronous C-ABI approach (Plugin ABI v2) is the right fix.** It's:
- Already proven in the Rivers codebase (Engine SDK uses the exact same pattern)
- The standard approach in the broader ecosystem (nginx, postgres, most libloading projects)
- The approach Tokio maintainers recommend for cdylib (Issues #1964, #6927 confirm the thread-local problem has no runtime fix)
- Simple to implement (JSON serialization is already used everywhere in Rivers)
- Future-proof (any language can implement C-ABI plugins)
- Crash-safe (`catch_unwind` works cleanly at C boundaries within the same compilation unit)

The shared dylib fix (v0.53.7) works as a bridge while v2 is implemented. The WASM approach is a longer-term evolution that could eventually replace both native and cdylib plugins.
