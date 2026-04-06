# Riverbed HTTPD Specification

**Document Type:** Implementation Specification  
**Scope:** Protocol engine, worker model, TLS, HTTP/1.1, HTTP/2, request/response lifecycle, error handling, hot reload, graceful shutdown  
**Status:** Reference / Ground Truth  
**Crate:** `riverbed-httpd`

---

## Table of Contents

1. [Design Philosophy](#1-design-philosophy)
2. [Dependencies](#2-dependencies)
3. [Architecture Overview](#3-architecture-overview)
4. [Worker Model](#4-worker-model)
5. [TLS and Protocol Negotiation](#5-tls-and-protocol-negotiation)
6. [Request Type](#6-request-type)
7. [Response Type](#7-response-type)
8. [Object Pooling](#8-object-pooling)
9. [Worker Loop State Machine](#9-worker-loop-state-machine)
10. [Dispatcher Trait](#10-dispatcher-trait)
11. [EngineFactory Trait](#11-enginefactory-trait)
12. [Error Handling](#12-error-handling)
13. [Lazy Body Reading](#13-lazy-body-reading)
14. [Response Drain Loops](#14-response-drain-loops)
15. [H1/H2 Normalization](#15-h1h2-normalization)
16. [Hot Reload](#16-hot-reload)
17. [Graceful Shutdown](#17-graceful-shutdown)
18. [Multiple Instances](#18-multiple-instances)
19. [Standalone Mode](#19-standalone-mode)
20. [Validation Rules](#20-validation-rules)

---

## 1. Design Philosophy

Riverbed is a protocol machine. It accepts TLS connections, negotiates HTTP/1.1 or HTTP/2 via ALPN, parses request headers, calls a consumer-provided dispatcher, writes responses, and manages connection lifecycle. It has zero opinions about routing, middleware, sessions, authentication, rate limiting, body limits, or application semantics.

All application behavior is injected via two traits: `Dispatcher` (handles requests) and `EngineFactory` (provides configuration policies). Riverbed never interprets the dispatcher's response — it writes whatever the dispatcher produces.

Riverbed is a standalone crate. It MAY be consumed by `riversd` for RESTful application traffic, by an RPS instance for provisioning traffic, or by any Rust application that needs a high-performance HTTP engine.

### 1.1 Ownership Boundary

Every section in this specification tags responsibilities explicitly:

- **riverbed MUST** — riverbed owns this behavior. The consumer does not implement, configure, or override it.
- **consumer MUST** — the consumer owns this behavior via the `Dispatcher` or `EngineFactory` traits.
- **riverbed MUST NOT** — riverbed is explicitly prohibited from this behavior.

### 1.2 Constraints

| ID | Rule |
|---|---|
| PHI-1 | Riverbed MUST NOT interpret request paths, method names, or query strings. |
| PHI-2 | Riverbed MUST NOT impose body size limits. The consumer provides limits via `EngineFactory`. |
| PHI-3 | Riverbed MUST NOT manage sessions, cookies, or authentication. |
| PHI-4 | Riverbed MUST NOT perform routing or request dispatch logic beyond calling `Dispatcher::dispatch()`. |
| PHI-5 | Riverbed MUST NOT format error response bodies for application errors. Engine-level errors (§12) produce minimal fixed responses. |
| PHI-6 | Riverbed MUST NOT compress responses. |
| PHI-7 | Riverbed MUST NOT set application-level headers (CORS, security headers, cache-control). |
| PHI-8 | Riverbed MUST validate structural HTTP protocol correctness: null bytes in headers, malformed request lines, header size overflow, content-length integrity, chunked encoding framing. |

---

## 2. Dependencies

Riverbed's dependency footprint is fixed. No additional runtime dependencies MAY be added without a spec amendment.

| Crate | Version | Purpose |
|---|---|---|
| `tokio` | 1.x | Async runtime (current_thread per worker), TCP listener, timer, mpsc channels |
| `tokio-rustls` | 0.26.x | TLS acceptor wrapping tokio TcpStream |
| `rustls` | 0.23.x | TLS configuration (passed in by consumer, not constructed by riverbed) |
| `httparse` | 1.x | Zero-copy HTTP/1.1 request parsing |
| `h2` | 0.4.x | HTTP/2 framing, HPACK, stream management |
| `bytes` | 1.x | `Bytes` and `BytesMut` for zero-copy buffer management (also transitive dep of `h2`) |
| `http` | 1.x | `http::HeaderMap`, `http::Method`, `http::StatusCode` shared types |
| `futures` | 0.3.x | `FutureExt::catch_unwind()` for panic isolation on async dispatch |

### 2.1 Constraints

| ID | Rule |
|---|---|
| DEP-1 | Riverbed MUST NOT depend on `axum`, `tower`, `tower-http`, `tower-service`, `tower-layer`, `hyper`, `hyper-util`, or `matchit`. |
| DEP-2 | Riverbed MUST NOT depend on any web framework crate. |
| DEP-3 | The `rustls::ServerConfig` is provided by the consumer. Riverbed MUST NOT construct TLS configurations or load certificates. |

---

## 3. Architecture Overview

```
riverbed::Server::start(tls_config, factory, worker_count)
  │
  │  returns ServerHandle (for shutdown and hot reload)
  │
  ├── Worker 0 (std::thread::spawn, own tokio current_thread runtime)
  │   ├── TcpListener bound with SO_REUSEPORT
  │   ├── TLS acceptor (tokio-rustls)
  │   ├── ALPN negotiation → H1 or H2 branch
  │   ├── Request pool (pre-allocated, Drop-recycled)
  │   ├── Response pool (pre-allocated, Drop-recycled)
  │   ├── Dispatcher (owned clone, stored in RefCell)
  │   ├── Reload receiver (mpsc::Receiver, checked on timer)
  │   └── Shutdown receiver (broadcast::Receiver)
  │
  ├── Worker 1..N (identical, independent)
  │
  └── Supervisor thread
      ├── holds mpsc::Sender<D> to each worker (hot reload broadcast)
      ├── holds broadcast::Sender (shutdown signal)
      └── ServerHandle::shutdown() → broadcast shutdown → join all workers
```

Riverbed MUST spawn all worker threads internally. The consumer calls `Server::start()` and receives a `ServerHandle`. The consumer MUST NOT manage threads.

### 3.1 Server Entry Point

```rust
pub struct Server;

impl Server {
    /// Starts the server. Spawns `worker_count` OS threads.
    /// Each thread binds its own TcpListener to `addr` with SO_REUSEPORT.
    /// Returns a handle for shutdown and hot reload.
    ///
    /// Riverbed MUST spawn workers via std::thread::spawn.
    /// Riverbed MUST build tokio::runtime::Builder::new_current_thread() per worker.
    /// Riverbed MUST NOT use a multi-threaded tokio runtime.
    pub fn start<D, F>(
        addr: std::net::SocketAddr,
        tls_config: Arc<rustls::ServerConfig>,
        factory: F,
        worker_count: usize,
    ) -> std::io::Result<ServerHandle<D>>
    where
        D: Dispatcher,
        F: EngineFactory<D>,
    {
        // ...
    }
}

pub struct ServerHandle<D: Dispatcher> {
    shutdown_tx: broadcast::Sender<()>,
    reload_txs: Vec<mpsc::Sender<D>>,
    worker_handles: Vec<std::thread::JoinHandle<()>>,
}
```

### 3.2 ServerHandle API

```rust
impl<D: Dispatcher> ServerHandle<D> {
    /// Signals all workers to begin graceful shutdown.
    /// Blocks until all worker threads have exited or drain timeout expires.
    /// Riverbed MUST block until all workers have exited.
    pub fn shutdown(self) {
        self.shutdown_with_timeout(Duration::from_secs(30));
    }

    /// Shutdown with a caller-specified drain timeout.
    /// In-flight requests that do not complete within the timeout are dropped.
    pub fn shutdown_with_timeout(self, timeout: Duration) {
        // ...
    }

    /// Broadcasts a new dispatcher to all workers.
    /// Each worker swaps its local dispatcher on the next reload timer tick.
    /// In-flight requests on the old dispatcher complete naturally.
    pub fn reload(&self, dispatcher: D) {
        // ...
    }
}
```

### 3.3 Constraints

| ID | Rule |
|---|---|
| ARCH-1 | Riverbed MUST spawn exactly `worker_count` OS threads via `std::thread::spawn`. |
| ARCH-2 | Each worker MUST build its own `tokio::runtime::Builder::new_current_thread()` runtime. |
| ARCH-3 | Each worker MUST bind its own `TcpListener` to `addr` with `SO_REUSEPORT` enabled. |
| ARCH-4 | Workers MUST NOT share any mutable state. Each worker is fully independent. |
| ARCH-5 | The consumer MUST NOT spawn worker threads or manage tokio runtimes. |
| ARCH-6 | `ServerHandle::shutdown()` MUST block until all workers have exited. |
| ARCH-7 | `ServerHandle::reload()` MUST send the new dispatcher to every worker. |

---

## 4. Worker Model

Each worker is a self-contained microserver. Workers do not share state, do not communicate with each other, and do not coordinate beyond receiving reload and shutdown signals from the supervisor.

### 4.1 Worker Internals

```rust
// Internal — not public API. Shown for implementation clarity.
struct Worker<D: Dispatcher> {
    listener: TcpListener,
    tls_acceptor: TlsAcceptor,
    dispatcher: RefCell<D>,
    request_pool: RefCell<ObjectPool<Request>>,
    response_pool: RefCell<ObjectPool<Response>>,
    reload_rx: mpsc::Receiver<D>,
    shutdown_rx: broadcast::Receiver<()>,
    config: WorkerConfig,
}

/// Extracted from EngineFactory at worker startup.
/// Riverbed MUST resolve all factory values once at startup
/// and store them in WorkerConfig.
struct WorkerConfig {
    body_limit: Option<usize>,
    keep_alive_timeout: Duration,
    max_connections: Option<usize>,
    idle_timeout: Duration,
    dispatch_timeout: Duration,
    pool_capacity: usize,
    stream_channel_capacity: usize,
    reload_check_interval: Duration,
}
```

### 4.2 Thread and Runtime Setup

```rust
// Per-worker thread startup (internal).
// Riverbed MUST use this pattern exactly.
std::thread::spawn(move || {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    rt.block_on(async move {
        worker.run().await;
    });
});
```

Riverbed MUST use `new_current_thread()`. Riverbed MUST NOT use `new_multi_thread()`. This guarantees all work on a worker thread stays on that thread — no cross-thread task migration, no work-stealing overhead, optimal cache locality.

### 4.3 SO_REUSEPORT Binding

```rust
// Per-worker listener binding (internal).
// Riverbed MUST use SO_REUSEPORT so each worker binds its own listener.
let socket = socket2::Socket::new(
    Domain::for_address(addr),
    Type::STREAM,
    Some(Protocol::TCP),
)?;
socket.set_reuse_port(true)?;
socket.set_reuse_address(true)?;
socket.set_nonblocking(true)?;
socket.bind(&addr.into())?;
socket.listen(1024)?;
let listener = TcpListener::from_std(socket.into())?;
```

The kernel distributes incoming connections across workers. No accept mutex, no lock contention.

Note: `socket2` is used here for `SO_REUSEPORT` configuration. It is a build-time dependency for socket setup only and does not appear in the hot path.

### 4.4 Constraints

| ID | Rule |
|---|---|
| WRK-1 | Each worker MUST run on its own OS thread via `std::thread::spawn`. |
| WRK-2 | Each worker MUST use `tokio::runtime::Builder::new_current_thread()`. |
| WRK-3 | Each worker MUST bind its own `TcpListener` with `SO_REUSEPORT`. |
| WRK-4 | Workers MUST NOT share mutable state. `RefCell` is safe because each worker is single-threaded. |
| WRK-5 | Each worker MUST own a cloned `Dispatcher` instance stored in `RefCell<D>`. |
| WRK-6 | Each worker MUST pre-allocate request and response object pools at startup. |
| WRK-7 | Worker MUST handle multiple concurrent async requests via `.await` interleaving on the single-threaded runtime. |

---

## 5. TLS and Protocol Negotiation

Riverbed MUST require TLS on all connections. There is no plaintext HTTP mode. The consumer provides a fully constructed `rustls::ServerConfig` with ALPN protocols configured.

### 5.1 ALPN Setup

The consumer MUST configure ALPN in the `ServerConfig` before passing it to riverbed. Typical configuration:

```rust
// Consumer responsibility — not riverbed code.
let mut config = rustls::ServerConfig::builder()
    .with_no_client_auth()  // or with_client_cert_verifier for mTLS
    .with_single_cert(cert_chain, private_key)?;
config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
```

### 5.2 Connection Accept Flow

```
TcpListener::accept()
    │
    ▼
TlsAcceptor::accept(tcp_stream).await
    │
    ├── TLS handshake failure → log, drop connection, continue accept loop
    │
    ▼
Extract negotiated ALPN protocol
    │
    ├── "h2"        → spawn H2 connection handler task
    ├── "http/1.1"  → spawn H1 connection handler task
    └── other/none  → default to H1
```

Riverbed MUST spawn each accepted connection as a separate tokio task on the worker's current-thread runtime. This allows concurrent connection handling within a single worker via async interleaving.

### 5.3 mTLS Support

Riverbed MUST NOT implement mTLS logic. The consumer configures `WebPkiClientVerifier` in the `ServerConfig` before passing it to riverbed. Riverbed accepts or rejects connections based on the TLS handshake result — it does not inspect client certificates.

### 5.4 Constraints

| ID | Rule |
|---|---|
| TLS-1 | Riverbed MUST require TLS on all connections. No plaintext mode. |
| TLS-2 | Riverbed MUST NOT construct `rustls::ServerConfig`. The consumer provides it. |
| TLS-3 | Riverbed MUST branch on negotiated ALPN protocol after TLS handshake. |
| TLS-4 | Riverbed MUST default to HTTP/1.1 if ALPN negotiation yields no protocol or an unrecognized protocol. |
| TLS-5 | Riverbed MUST spawn each accepted connection as a separate tokio task on the worker's current-thread runtime. |
| TLS-6 | TLS handshake failures MUST be logged and the connection dropped. The accept loop MUST continue. |

---

## 6. Request Type

`Request` is a riverbed-owned opaque type. The consumer receives `&Request` for reading and calls methods on it to access body content. The consumer MUST NOT construct `Request` instances.

### 6.1 Type Definition

```rust
pub struct Request {
    method: String,
    path: String,
    headers: http::HeaderMap,
    body: BodyReader,
    body_consumed: BodyConsumeState,
}

pub enum BodyReader {
    H1 {
        read_half: tokio::io::ReadHalf<tokio_rustls::server::TlsStream<TcpStream>>,
        content_length: Option<u64>,
        chunked: bool,
        bytes_read: u64,
    },
    H2(h2::RecvStream),
    Empty,
}

#[derive(Clone, Copy, PartialEq)]
pub enum BodyConsumeState {
    Untouched,
    Partial,
    Consumed,
}
```

### 6.2 Public API

```rust
impl Request {
    /// Returns the HTTP method as a string slice.
    /// Riverbed MUST NOT validate method names.
    /// The consumer decides which methods are acceptable.
    pub fn method(&self) -> &str { &self.method }

    /// Returns the request path as a string slice.
    /// Includes query string if present (e.g., "/api/users?id=5").
    /// Riverbed MUST NOT validate path semantics.
    /// The consumer decides which paths are valid and parses query parameters.
    pub fn path(&self) -> &str { &self.path }

    /// Returns a reference to the header map.
    pub fn headers(&self) -> &http::HeaderMap { &self.headers }

    /// Reads the full request body into a Bytes buffer.
    /// Respects the body limit from EngineFactory if set.
    /// Returns EngineError::BodyLimitExceeded if the body exceeds the limit.
    /// Returns an error if body was already consumed.
    /// Sets body_consumed to Consumed on success.
    /// Riverbed MUST track consumption state.
    pub async fn body(&mut self) -> Result<Bytes, EngineError> { /* ... */ }

    /// Returns an async reader for incremental body consumption.
    /// Sets body_consumed to Partial on first read, Consumed when exhausted.
    /// Riverbed MUST track consumption state.
    pub fn body_stream(&mut self) -> BodyStream<'_> { /* ... */ }
}
```

### 6.3 Reset for Pool Recycling

```rust
impl Request {
    /// Riverbed MUST call reset() before returning to the pool.
    /// Clears all fields to default state.
    fn reset(&mut self) {
        self.method.clear();
        self.path.clear();
        self.headers.clear();
        self.body = BodyReader::Empty;
        self.body_consumed = BodyConsumeState::Untouched;
    }
}
```

### 6.4 Constraints

| ID | Rule |
|---|---|
| REQ-1 | `Request` is opaque. The consumer MUST NOT construct `Request` instances. |
| REQ-2 | Riverbed MUST NOT validate method names. The consumer decides validity. |
| REQ-3 | Riverbed MUST NOT validate path semantics. The consumer decides validity. |
| REQ-4 | Riverbed MUST validate structural HTTP correctness: null bytes in headers, malformed request lines, content-length integrity, chunked framing. |
| REQ-5 | Riverbed MUST track `BodyConsumeState` across `body()` and `body_stream()` calls. |
| REQ-6 | Riverbed MUST enforce `EngineFactory::body_limit()` during body reads. |
| REQ-7 | Riverbed MUST call `reset()` before returning a `Request` to the pool. |

---

## 7. Response Type

`Response` is a riverbed-owned opaque type. The consumer receives `&mut Response` during dispatch and sets status, headers, and body variant.

### 7.1 Type Definition

```rust
pub struct Response {
    status: u16,
    headers: http::HeaderMap,
    body: ResponseBody,
}

pub enum ResponseBody {
    /// Complete body. Worker writes in one operation.
    Fixed(Bytes),

    /// Streaming body. Worker drains the receiver, writing chunks as they arrive.
    /// Channel close (sender drop) signals stream completion.
    /// Channel capacity is configured via EngineFactory::stream_channel_capacity().
    Stream(mpsc::Receiver<Bytes>),

    /// Protocol upgrade (WebSocket). Worker hands off the raw TLS stream.
    /// The connection is removed from worker management entirely.
    Upgrade(Box<dyn FnOnce(tokio_rustls::server::TlsStream<TcpStream>) + Send>),
}
```

### 7.2 Public API

```rust
impl Response {
    /// Sets the HTTP status code.
    pub fn set_status(&mut self, status: u16) { self.status = status; }

    /// Returns the current status code.
    pub fn status(&self) -> u16 { self.status }

    /// Returns a reference to the response header map.
    pub fn headers(&self) -> &http::HeaderMap { &self.headers }

    /// Returns a mutable reference to the response header map.
    pub fn headers_mut(&mut self) -> &mut http::HeaderMap { &mut self.headers }

    /// Sets the response body to a fixed byte buffer.
    pub fn set_body_fixed(&mut self, body: Bytes) {
        self.body = ResponseBody::Fixed(body);
    }

    /// Sets the response body to a streaming channel.
    /// The consumer provides the Receiver side.
    /// The consumer retains the Sender and pushes chunks.
    /// The consumer MUST drop the Sender when streaming is complete.
    pub fn set_body_stream(&mut self, rx: mpsc::Receiver<Bytes>) {
        self.body = ResponseBody::Stream(rx);
    }

    /// Sets the response body to a protocol upgrade handler.
    /// Worker writes 101 Switching Protocols, then calls the handler
    /// with the raw TLS stream. Connection is removed from worker management.
    pub fn set_body_upgrade(
        &mut self,
        handler: Box<dyn FnOnce(tokio_rustls::server::TlsStream<TcpStream>) + Send>,
    ) {
        self.body = ResponseBody::Upgrade(handler);
    }
}
```

### 7.3 Default State

Riverbed MUST initialize `Response` with:

- `status = 200`
- `headers` = empty `HeaderMap`
- `body = ResponseBody::Fixed(Bytes::new())` (empty body)

### 7.4 Reset for Pool Recycling

```rust
impl Response {
    /// Riverbed MUST call reset() before returning to the pool.
    fn reset(&mut self) {
        self.status = 200;
        self.headers.clear();
        self.body = ResponseBody::Fixed(Bytes::new());
    }
}
```

### 7.5 Constraints

| ID | Rule |
|---|---|
| RES-1 | `Response` is opaque. The consumer MUST NOT construct `Response` instances. |
| RES-2 | Riverbed MUST initialize `Response` to status 200 with empty headers and empty Fixed body. |
| RES-3 | The consumer MUST drop the `mpsc::Sender` to signal stream completion. |
| RES-4 | Riverbed MUST call `reset()` before returning a `Response` to the pool. |
| RES-5 | Riverbed MUST remove upgraded connections from worker management. No keep-alive after upgrade. |

---

## 8. Object Pooling

Riverbed pre-allocates `Request` and `Response` objects per worker at startup. Objects are checked out on each request and returned after the request completes.

### 8.1 Pool Structure

```rust
// Thread-local pool — internal to each worker.
struct ObjectPool<T: Poolable> {
    objects: Vec<T>,
    capacity: usize,
}

impl<T: Poolable> ObjectPool<T> {
    /// Pre-allocate `capacity` objects.
    fn new(capacity: usize) -> Self {
        let mut objects = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            objects.push(T::new_default());
        }
        ObjectPool { objects, capacity }
    }

    /// Check out an object. Returns a pre-allocated object if available,
    /// or allocates a new one if the pool is empty.
    fn checkout(&mut self) -> T {
        self.objects.pop().unwrap_or_else(|| T::new_default())
    }

    /// Return an object to the pool. Calls reset() before storing.
    /// If the pool is at capacity, the object is dropped instead.
    fn checkin(&mut self, mut obj: T) {
        if self.objects.len() < self.capacity {
            obj.reset();
            self.objects.push(obj);
        }
        // else: drop — pool is full
    }
}

trait Poolable {
    fn new_default() -> Self;
    fn reset(&mut self);
}
```

### 8.2 Checkout and Return Sequence

Riverbed MUST NOT use `Drop`-based auto-return. The worker MUST explicitly control the return sequence because post-dispatch cleanup (body drain, response write) must complete before recycling.

```
1. Checkout Request and Response from pools
2. Fill Request from parsed HTTP data
3. Dispatch (may panic — caught by catch_unwind)
4. Drain unconsumed body if needed (§13)
5. Write response via drain loop (§14)
6. Checkin Request and Response to pools
```

On error paths, the sequence is:

```
1. Checkout Request and Response from pools
2. Fill Request from parsed HTTP data
3. Dispatch fails or panics
4. Skip body drain on panic (ERR-7), drain on other errors
5. Write engine error response
6. Checkin Request and Response to pools
```

Objects MUST be returned to the pool on every path — success, error, panic.

### 8.3 Constraints

| ID | Rule |
|---|---|
| POOL-1 | Riverbed MUST pre-allocate `pool_capacity` Request and Response objects per worker at startup. |
| POOL-2 | Pool checkout MUST return a pre-allocated object or allocate a new one if the pool is empty. |
| POOL-3 | Pool checkin MUST call `reset()` on the object before storing. |
| POOL-4 | Pool checkin MUST drop the object if the pool is at capacity. |
| POOL-5 | Pools are thread-local (`RefCell<ObjectPool<T>>`). No cross-thread access. |
| POOL-6 | Pool capacity is set via `EngineFactory::pool_capacity()`. |
| POOL-7 | Riverbed MUST NOT use `Drop`-based auto-return. Worker controls the return sequence explicitly. |

---

## 9. Worker Loop State Machine

The worker loop is the core of riverbed. Every state, every transition, and every error branch is defined here.

### 9.1 Connection Accept Loop

```
                    ┌──────────────┐
                    │              │
                    ▼              │
              ┌──────────┐        │
              │  SELECT  │        │
              └────┬─────┘        │
                   │              │
         ┌─────────┴──────────┐   │
         │                    │   │
         ▼                    ▼   │
    ┌─────────┐          ┌──────┐ │
    │ ACCEPT  │          │SHUTDN│ │
    │ new conn│          │signal│ │
    └────┬────┘          └──┬───┘ │
         │                  │     │
         │                  ▼     │
         │            ┌───────┐   │
         │            │ DRAIN │   │
         │            │& EXIT │   │
         │            └───────┘   │
         ▼                        │
   ┌────────────┐                 │
   │ TLS_ACCEPT │                 │
   └─────┬──────┘                 │
         │                        │
    ┌────┴────┐                   │
    │         │                   │
    ▼         ▼                   │
  fail     success                │
    │         │                   │
    ▼         ▼                   │
  log      ┌──────┐              │
  drop     │ ALPN │              │
  ──────►  │CHECK │              │
  continue └──┬───┘              │
              │                  │
        ┌─────┴─────┐           │
        ▼           ▼           │
    ┌──────┐    ┌──────┐        │
    │  H1  │    │  H2  │        │
    │ task │    │ task │        │
    └──┬───┘    └──┬───┘        │
       │           │            │
       └───────────┴────────────┘
              (loop back to SELECT)
```

Riverbed MUST use `tokio::select!` on accept and shutdown only. Reload is handled on a separate timer task (§16), not in the accept `select!`.

### 9.2 Accept Loop — select! Structure

```rust
// Internal worker accept loop.
loop {
    tokio::select! {
        // Accept new connection
        result = self.listener.accept() => {
            match result {
                Ok((tcp_stream, _addr)) => {
                    // Check max_connections if configured
                    // Spawn connection handler as separate task
                    let tls_acceptor = self.tls_acceptor.clone();
                    let dispatcher = self.dispatcher.borrow().clone();
                    tokio::task::spawn_local(async move {
                        handle_connection(tcp_stream, tls_acceptor, dispatcher, config).await;
                    });
                }
                Err(e) => {
                    // Log accept error, continue loop
                }
            }
        }

        // Shutdown signal
        _ = self.shutdown_rx.recv() => {
            // Stop accepting. In-flight tasks complete naturally.
            break;
        }
    }
}
```

### 9.3 Per-Request State Machine (H1)

```
┌──────────────────┐
│ READ_REQUEST_LINE │  ← read from socket into parse buffer
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  PARSE_HEADERS   │  ← httparse::Request::parse(&buf)
└────────┬─────────┘
         │
    ┌────┴────┐
    │         │
    ▼         ▼
 incomplete  complete
    │         │
    ▼         │
 read more   │
 loop back   │
              ▼
┌──────────────────┐
│ VALIDATE_STRUCT  │  ← structural HTTP validation (§20.1)
└────────┬─────────┘
         │
    ┌────┴────┐
    │         │
    ▼         ▼
  invalid   valid
    │         │
    ▼         │
  400        │
  close      │
              ▼
┌──────────────────┐
│ CHECKOUT_OBJECTS │  ← request_pool.checkout(), response_pool.checkout()
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  FILL_REQUEST    │  ← fill_from_h1(&mut req, parsed)
└────────┬─────────┘
         │
         ▼
┌──────────────────────────────────────────────────────┐
│ DISPATCH                                             │
│                                                      │
│ timeout(                                             │
│   dispatch_timeout,                                  │
│   AssertUnwindSafe(dispatcher.dispatch(&req, &mut res))│
│     .catch_unwind()                                  │
│ )                                                    │
└────────┬─────────────────────────────────────────────┘
         │
    ┌────┴──────────────────┐
    │         │              │
    ▼         ▼              ▼
  Ok(Ok(()))  Ok(Err(e))   Err(panic)
    │         │              │
    │         │              ▼
    │         │        EngineError::
    │         │        DispatchPanicked
    │         │              │
    │         ▼              │
    │    EngineError         │
    │    from dispatch       │
    │         │              │
    ▼         ▼              ▼
┌──────────┐ ┌───────────────────┐
│DRAIN_BODY│ │  ENGINE_ERROR     │
│if needed │ │  write error resp │
└────┬─────┘ │  close connection │
     │       │  recycle objects  │
     ▼       └───────────────────┘
┌──────────────────┐
│ WRITE_RESPONSE   │  ← variant-specific drain loop (§14)
└────────┬─────────┘
         │
    ┌────┴────┐
    │         │
    ▼         ▼
  error    success
    │         │
    ▼         ▼
  close  ┌──────────┐
  recycle│ CHECKIN   │  ← reset & return objects to pool
         └────┬─────┘
              │
              ▼
         ┌──────────┐
         │KEEP_ALIVE│  ← Connection: keep-alive → loop to READ_REQUEST_LINE
         └────┬─────┘   idle_timeout exceeded or Connection: close → close
              │
         ┌────┴────┐
         │         │
         ▼         ▼
       reuse     close
       loop      done
```

### 9.4 Per-Request State Machine (H2)

```
┌──────────────────┐
│ H2_HANDSHAKE     │  ← h2::server::handshake(tls_stream).await
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ ACCEPT_STREAM    │  ← connection.accept().await → Option<(request, send_response)>
└────────┬─────────┘
         │
    ┌────┴────┐
    │         │
    ▼         ▼
   None     Some
    │         │
    ▼         │
  conn        │
  done        ▼
         ┌──────────────────┐
         │ SPAWN_STREAM_TASK│  ← tokio::task::spawn_local for this stream
         └────────┬─────────┘
                  │ (accept loop continues immediately for next stream)
                  ▼
         ┌──────────────────┐
         │ CHECKOUT_OBJECTS │  ← request_pool.checkout(), response_pool.checkout()
         └────────┬─────────┘
                  │
                  ▼
         ┌──────────────────┐
         │  FILL_REQUEST    │  ← fill_from_h2(&mut req, h2_request)
         └────────┬─────────┘
                  │
                  ▼
         ┌──────────────────┐
         │    DISPATCH      │  ← same catch_unwind + timeout as H1
         └────────┬─────────┘
                  │
             ┌────┴────────────┐
             │                 │
             ▼                 ▼
           Ok(())        Err(EngineError)
             │                 │
             ▼                 ▼
         ┌──────────┐   ┌───────────────┐
         │DRAIN_BODY│   │ ENGINE_ERROR  │
         │if needed │   │ send error    │
         └────┬─────┘   │ recycle       │
              │         └───────────────┘
              ▼
         ┌──────────────────┐
         │  SEND_RESPONSE   │  ← h2 send_response + send_data
         └────────┬─────────┘
                  │
                  ▼
         ┌──────────────────┐
         │    CHECKIN        │  ← reset & return objects to pool
         └──────────────────┘
```

H2 multiplexes streams on a single connection. Each stream is a separate task. The connection handler loop (`ACCEPT_STREAM`) continues accepting new streams while existing stream tasks execute concurrently. Keep-alive is inherent — H2 connections persist until either side sends GOAWAY.

### 9.5 Constraints

| ID | Rule |
|---|---|
| WKL-1 | Riverbed MUST use `tokio::select!` for accept and shutdown only. Reload MUST NOT be in the accept select. |
| WKL-2 | Each accepted connection MUST be spawned as a separate tokio task via `spawn_local`. |
| WKL-3 | H1: Riverbed MUST loop `httparse::Request::parse()` until `Status::Complete` or buffer overflow. |
| WKL-4 | H1: Structural validation failure MUST return the appropriate 4xx status and close the connection. |
| WKL-5 | H1: Riverbed MUST call `fill_from_h1()` to normalize parsed output into `Request`. |
| WKL-6 | H2: Riverbed MUST call `h2::server::handshake()` on the TLS stream. |
| WKL-7 | H2: Each H2 stream MUST be spawned as a separate tokio task via `spawn_local` within the connection task. |
| WKL-8 | H2: Riverbed MUST call `fill_from_h2()` to normalize H2 headers into `Request`. |
| WKL-9 | Riverbed MUST drain unconsumed body before keep-alive reuse (§13). |
| WKL-10 | Riverbed MUST checkin Request and Response objects after every request, including error paths. |
| WKL-11 | H1: Keep-alive MUST be honored. If `Connection: keep-alive` (or HTTP/1.1 default), loop to `READ_REQUEST_LINE`. |
| WKL-12 | H1: Keep-alive idle timeout MUST be enforced per `EngineFactory::idle_timeout()`. |
| WKL-13 | Dispatch MUST be wrapped in `tokio::time::timeout(dispatch_timeout, ...)`. Timeout → `EngineError::Timeout`. |
| WKL-14 | Dispatch MUST be wrapped in `AssertUnwindSafe(...).catch_unwind()`. Panic → `EngineError::DispatchPanicked`. |

---

## 10. Dispatcher Trait

The `Dispatcher` trait is the sole integration point between riverbed and the consumer. Riverbed calls `dispatch()` for every request. The consumer implements all application logic inside this method.

### 10.1 Trait Definition

```rust
pub trait Dispatcher: Clone + Send + 'static {
    /// Called by riverbed for every parsed request.
    ///
    /// Riverbed provides `&self` — the dispatcher MUST use interior mutability
    /// (RefCell, Cell) for any per-worker mutable state. This is safe because
    /// each worker is single-threaded (current_thread tokio runtime).
    ///
    /// Multiple dispatch() calls MAY be in flight concurrently on the same
    /// worker via async interleaving at .await points. The dispatcher MUST
    /// be safe for concurrent async calls with &self.
    ///
    /// The consumer reads from &Request and writes into &mut Response.
    /// The consumer MUST set res.status, res.headers, and res.body as needed.
    /// If the consumer does not modify the response, the default (200, empty body) is used.
    ///
    /// Returning Ok(()) tells riverbed to write the response as-is.
    /// Returning Err(EngineError) tells riverbed to handle the error (§12).
    fn dispatch(
        &self,
        req: &Request,
        res: &mut Response,
    ) -> impl Future<Output = Result<(), EngineError>> + Send;
}
```

### 10.2 Clone Semantics

Each worker receives its own clone of the dispatcher. `Clone` MUST produce an independent instance.

- Shared state (connection pools, event bus handles, view registry) SHOULD be behind `Arc` within the dispatcher.
- Per-worker mutable state (rate limiter counters, local metrics) SHOULD use `RefCell` or `Cell`.

### 10.3 Constraints

| ID | Rule |
|---|---|
| DSP-1 | Riverbed MUST call `dispatch()` with `&self` (immutable borrow). |
| DSP-2 | The consumer MUST use `Clone` to produce per-worker dispatcher instances. |
| DSP-3 | The consumer MUST use interior mutability (`RefCell`, `Cell`) for per-worker mutable state. |
| DSP-4 | Riverbed MUST wrap `dispatch()` in `AssertUnwindSafe` + `FutureExt::catch_unwind()`. |
| DSP-5 | A panic in `dispatch()` MUST produce `EngineError::DispatchPanicked`. |
| DSP-6 | Riverbed MUST NOT interpret the response set by the dispatcher. It writes whatever the dispatcher produces. |
| DSP-7 | Riverbed MUST enforce `EngineFactory::dispatch_timeout()` via `tokio::time::timeout()` wrapping the dispatch call. |

---

## 11. EngineFactory Trait

The `EngineFactory` trait provides policy configuration to riverbed. The consumer implements this trait to control engine behavior without modifying riverbed code.

### 11.1 Trait Definition

```rust
pub trait EngineFactory<D: Dispatcher>: Send + Sync + 'static {
    /// Creates a dispatcher instance.
    /// Called once per worker at startup.
    /// The returned dispatcher is cloned per-worker.
    fn dispatcher(&self) -> D;

    /// Maximum request body size in bytes. None = unlimited.
    /// Enforced during Request::body() and Request::body_stream() reads.
    fn body_limit(&self) -> Option<usize>;

    /// Keep-alive timeout for connections.
    /// Connections idle longer than this between requests are closed.
    fn keep_alive_timeout(&self) -> Duration;

    /// Maximum concurrent connections per worker. None = unlimited.
    fn max_connections(&self) -> Option<usize>;

    /// Idle timeout for H1 keep-alive connections waiting for the next request.
    fn idle_timeout(&self) -> Duration;

    /// Maximum wall clock time for a single dispatch() call.
    /// Exceeded → EngineError::Timeout.
    fn dispatch_timeout(&self) -> Duration;

    /// Number of Request and Response objects pre-allocated per worker pool.
    fn pool_capacity(&self) -> usize;

    /// Bounded channel capacity for ResponseBody::Stream.
    /// Controls backpressure between the dispatcher's sender and the worker's drain loop.
    fn stream_channel_capacity(&self) -> usize;

    /// Interval between hot reload channel checks.
    /// Recommended default: 250ms.
    fn reload_check_interval(&self) -> Duration;
}
```

### 11.2 Constraints

| ID | Rule |
|---|---|
| FAC-1 | Riverbed MUST call each factory method exactly once per worker at startup and cache the results in `WorkerConfig`. |
| FAC-2 | Riverbed MUST NOT call factory methods on the hot path. All values are resolved at startup. |
| FAC-3 | `body_limit()` MUST be enforced in `Request::body()` and `Request::body_stream()`. |
| FAC-4 | `dispatch_timeout()` MUST be enforced via `tokio::time::timeout()` wrapping `dispatch()`. |
| FAC-5 | `pool_capacity()` MUST control pre-allocation count at worker startup. |
| FAC-6 | `stream_channel_capacity()` MUST be used as the capacity for bounded `mpsc::channel()` when streaming. |
| FAC-7 | `reload_check_interval()` MUST control the timer interval for hot reload checks (§16). |
| FAC-8 | `idle_timeout()` MUST close H1 keep-alive connections that have been idle for this duration. |

---

## 12. Error Handling

Riverbed defines engine-level errors. Application-level errors are the consumer's responsibility and flow through the normal `Response` path (status codes, headers, body set by the dispatcher).

### 12.1 EngineError Definition

```rust
#[derive(Debug)]
pub enum EngineError {
    /// Socket read failed during lazy body read.
    BodyReadFailed(std::io::Error),

    /// Request body exceeded EngineFactory::body_limit().
    /// Contains the configured limit for diagnostics.
    BodyLimitExceeded(usize),

    /// Socket write failed during response streaming.
    StreamWriteFailed(std::io::Error),

    /// Dispatcher panicked. Caught by catch_unwind.
    DispatchPanicked,

    /// Dispatch call exceeded EngineFactory::dispatch_timeout().
    Timeout,

    /// Consumer-defined error with explicit status and connection policy.
    Custom(CustomError),
}
```

### 12.2 CustomError

```rust
pub struct CustomError {
    pub status: u16,
    pub close_connection: bool,
    pub body: Option<Bytes>,
    source: Box<dyn std::error::Error + Send + 'static>,
}

impl CustomError {
    /// Create a new custom error.
    /// Defaults: close_connection = false, body = None.
    pub fn new(status: u16, source: impl std::error::Error + Send + 'static) -> Self {
        CustomError {
            status,
            close_connection: false,
            body: None,
            source: Box::new(source),
        }
    }

    /// Set close_connection to true.
    pub fn close(mut self) -> Self {
        self.close_connection = true;
        self
    }

    /// Set the response body.
    pub fn with_body(mut self, body: Bytes) -> Self {
        self.body = Some(body);
        self
    }

    /// Returns the source error for logging and error chaining.
    pub fn source(&self) -> &(dyn std::error::Error + Send + 'static) {
        &*self.source
    }
}
```

### 12.3 Worker Error Response Behavior

| Error | HTTP Status | Close Connection | Response Body |
|---|---|---|---|
| `BodyReadFailed` | 400 | Yes | `{"error": "bad request"}` |
| `BodyLimitExceeded` | 413 | Yes | `{"error": "payload too large"}` |
| `StreamWriteFailed` | — | Yes | None (client is gone) |
| `DispatchPanicked` | 500 | Yes | `{"error": "internal server error"}` |
| `Timeout` | 504 | Yes | `{"error": "gateway timeout"}` |
| `Custom(c)` | `c.status` | `c.close_connection` | `c.body` or empty |

### 12.4 Constraints

| ID | Rule |
|---|---|
| ERR-1 | All built-in `EngineError` variants (not Custom) MUST close the connection. |
| ERR-2 | `Custom` errors MUST respect `close_connection`. If `false`, keep-alive MAY continue. |
| ERR-3 | Riverbed MUST write error responses as `Content-Type: application/json` for built-in errors. |
| ERR-4 | Riverbed MUST NOT write a response for `StreamWriteFailed` — the client is gone. |
| ERR-5 | `BodyLimitExceeded` MUST include the configured limit value for diagnostics. |
| ERR-6 | Riverbed MUST checkin Request and Response objects after every error path. |
| ERR-7 | Body drain MUST be skipped on `DispatchPanicked` — the connection state is unknown. |

---

## 13. Lazy Body Reading

Riverbed parses request headers only. The body remains on the socket until the dispatcher explicitly reads it. This allows the dispatcher to reject requests (auth failure, bad route) without consuming the body.

### 13.1 Body Read Flow

```
Dispatcher calls req.body().await
    │
    ▼
Check BodyConsumeState
    │
    ├── Consumed → return Err (already consumed)
    │
    ├── Partial → return Err (partial read via stream, cannot buffer retroactively)
    │
    └── Untouched
         │
         ▼
    Read from BodyReader
         │
         ├── BodyReader::Empty → return Ok(Bytes::new())
         │
         ├── BodyReader::H1
         │   ├── content_length set → read exactly N bytes
         │   │   ├── check body_limit during read
         │   │   └── EngineError::BodyLimitExceeded if exceeded
         │   ├── chunked = true → read and decode chunked transfer encoding
         │   │   ├── check body_limit during read
         │   │   └── EngineError::BodyLimitExceeded if exceeded
         │   └── neither → read until connection close (H1 legacy, not recommended)
         │
         └── BodyReader::H2
             └── h2::RecvStream::data().await until end of stream
                 ├── check body_limit during read
                 └── EngineError::BodyLimitExceeded if exceeded
         │
         ▼
    Set body_consumed = Consumed
         │
         ▼
    Return Ok(body_bytes)
```

### 13.2 Unconsumed Body Drain

After `dispatch()` returns, the worker MUST drain any unconsumed body before recycling the connection for H1 keep-alive.

```
Check body_consumed
    │
    ├── Consumed → no drain needed
    │
    ├── Untouched
    │   ├── BodyReader::Empty → no drain needed
    │   ├── BodyReader::H1
    │   │   ├── content_length set → read and discard exactly N bytes
    │   │   ├── chunked = true → read and discard until chunk terminator (0\r\n\r\n)
    │   │   └── neither → close connection (cannot determine body boundary)
    │   └── BodyReader::H2 → RecvStream drains automatically on drop
    │
    └── Partial → drain remaining bytes (same as Untouched for the remaining portion)
```

If draining fails (socket error, timeout), the worker MUST close the connection. No keep-alive.

### 13.3 Drain Timeout

Riverbed MUST apply a fixed 5-second drain timeout to prevent a malicious client from holding a connection open by sending body bytes slowly. This is a protocol-level safety measure. It is NOT factory-configurable — slow body delivery during drain is always adversarial.

### 13.4 Constraints

| ID | Rule |
|---|---|
| LBR-1 | Riverbed MUST parse headers only. The body MUST remain on the socket until the dispatcher reads it. |
| LBR-2 | `Request::body()` MUST enforce `EngineFactory::body_limit()` during reads. |
| LBR-3 | `Request::body()` on an already-consumed body MUST return an error. |
| LBR-4 | After dispatch returns, riverbed MUST drain unconsumed body bytes before H1 keep-alive reuse. |
| LBR-5 | If body drain fails, riverbed MUST close the connection. No keep-alive. |
| LBR-6 | Riverbed MUST apply a fixed 5-second drain timeout. |
| LBR-7 | H2 body drains automatically when `RecvStream` is dropped. Riverbed MUST still track `BodyConsumeState` for H2. |
| LBR-8 | Body drain MUST be skipped on `DispatchPanicked` — connection state is unknown, close immediately. |

---

## 14. Response Drain Loops

The worker owns the response write path. Each `ResponseBody` variant has an explicit drain loop. The worker always knows the exit condition.

### 14.1 Fixed Body

```
WRITE_FIXED:
    1. Format HTTP/1.1 status line + headers into write buffer
    2. Set Content-Length header to body.len()
    3. Write headers to socket
    4. Write body bytes to socket
    5. Flush
    │
    ├── write error → log, close connection, checkin objects
    └── success → checkin objects, evaluate keep-alive
```

For H2: use `send_response()` to send headers, then `send_data()` with `end_of_stream = true` for the body.

### 14.2 Stream Body

```
WRITE_STREAM:
    1. Format HTTP/1.1 status line + headers into write buffer
    2. Set Transfer-Encoding: chunked (H1 only; H2 uses DATA frames natively)
    3. Write headers to socket
    4. Enter drain loop:
       │
       loop {
           match receiver.recv().await {
               Some(chunk) →
                   │
                   ├── Write chunk to socket
                   │   (H1: chunk-size\r\n + data + \r\n)
                   │   (H2: send_data with end_of_stream = false)
                   │
                   │   ├── write error → log, drop receiver, close connection, checkin objects
                   │   └── success → continue loop
                   │
               None →
                   │  (channel closed — sender dropped — stream complete)
                   │
                   ├── H1: Write terminal chunk "0\r\n\r\n"
                   ├── H2: send_data with end_of_stream = true (empty)
                   └── break
           }
       }
    5. Flush
    │
    └── success → checkin objects, evaluate keep-alive
```

### 14.3 Upgrade

```
WRITE_UPGRADE:
    1. Format HTTP/1.1 101 Switching Protocols + headers
    2. Write headers to socket
    3. Flush
    4. Extract raw TLS stream from connection
    5. Call upgrade_handler(tls_stream)
    6. Connection is gone — removed from worker management
    7. Checkin Request and Response objects only (NOT the connection)
```

Upgrade is H1 only. H2 does not support the 101 Switching Protocols mechanism. If the dispatcher sets `ResponseBody::Upgrade` on an H2 stream, riverbed MUST return `EngineError::Custom` with status 501 (Not Implemented).

### 14.4 Constraints

| ID | Rule |
|---|---|
| DRN-1 | Fixed: Riverbed MUST set `Content-Length` from body length. |
| DRN-2 | Stream H1: Riverbed MUST set `Transfer-Encoding: chunked`. |
| DRN-3 | Stream H2: Riverbed MUST use `send_data()` DATA frames. No chunked encoding on H2. |
| DRN-4 | Stream: Riverbed MUST drain the receiver until `None` (channel close). |
| DRN-5 | Stream: Write error MUST cause the receiver to be dropped, signaling the sender. |
| DRN-6 | Upgrade: Riverbed MUST write 101 status before calling the upgrade handler. |
| DRN-7 | Upgrade: The connection MUST be removed from worker management after upgrade. |
| DRN-8 | Upgrade: Request and Response objects MUST still be checkin'd after upgrade. |
| DRN-9 | All drain loops MUST checkin objects on both success and error paths. |
| DRN-10 | Upgrade on H2 MUST return an error. H2 does not support 101 Switching Protocols. |

---

## 15. H1/H2 Normalization

Two protocol parsers produce the same `Request` type. This section defines the exact field mapping so both code paths converge to identical `Request` state.

### 15.1 fill_from_h1

```rust
/// Populates a pool-checked-out Request from httparse output.
/// Riverbed MUST call this after successful httparse::Request::parse().
///
/// httparse produces borrowed &str slices from the parse buffer.
/// This function copies them into Request's owned fields.
fn fill_from_h1(
    req: &mut Request,
    parsed: &httparse::Request<'_, '_>,
    read_half: ReadHalf,
    content_length: Option<u64>,
    chunked: bool,
) {
    // Method: copy from parsed.method
    req.method.clear();
    req.method.push_str(parsed.method.unwrap());

    // Path: copy from parsed.path (includes query string)
    req.path.clear();
    req.path.push_str(parsed.path.unwrap());

    // Headers: iterate parsed.headers, insert into HeaderMap
    req.headers.clear();
    for header in parsed.headers.iter() {
        let name = http::header::HeaderName::from_bytes(header.name.as_bytes());
        let value = http::header::HeaderValue::from_bytes(header.value);
        if let (Ok(n), Ok(v)) = (name, value) {
            req.headers.append(n, v);
        }
        // Invalid header names/values are silently dropped.
        // Structural validation (§20.1) catches malformed headers
        // before this function is called.
    }

    // Body reader: lazy — body stays on socket
    req.body = BodyReader::H1 {
        read_half,
        content_length,
        chunked,
        bytes_read: 0,
    };
    req.body_consumed = BodyConsumeState::Untouched;
}
```

### 15.2 fill_from_h2

```rust
/// Populates a pool-checked-out Request from an h2 request.
/// Riverbed MUST call this when a new H2 stream is received.
///
/// h2 provides owned types — method and URI are extracted from http::Request parts.
fn fill_from_h2(
    req: &mut Request,
    h2_request: http::Request<h2::RecvStream>,
) {
    let (parts, recv_stream) = h2_request.into_parts();

    // Method: from http::Method to String
    req.method.clear();
    req.method.push_str(parts.method.as_str());

    // Path: from http::Uri — path + query
    req.path.clear();
    if let Some(pq) = parts.uri.path_and_query() {
        req.path.push_str(pq.as_str());
    }

    // Headers: direct transfer — both use http::HeaderMap
    req.headers.clear();
    req.headers.extend(parts.headers.into_iter());

    // Body reader: h2::RecvStream is already lazy
    req.body = BodyReader::H2(recv_stream);
    req.body_consumed = BodyConsumeState::Untouched;
}
```

### 15.3 Field Mapping Table

| Field | H1 Source | H2 Source | Conversion |
|---|---|---|---|
| `method` | `httparse::Request::method` (`Option<&str>`) | `http::request::Parts::method` (`http::Method`) | Copy into owned `String` |
| `path` | `httparse::Request::path` (`Option<&str>`) | `http::request::Parts::uri.path_and_query()` (`Option<&PathAndQuery>`) | Copy into owned `String` |
| `headers` | `httparse::Header[]` (`name: &str`, `value: &[u8]`) | `http::request::Parts::headers` (`http::HeaderMap`) | Copy/extend into owned `HeaderMap` |
| `body` | `BodyReader::H1 { read_half, ... }` | `BodyReader::H2(RecvStream)` | Owned handle to socket/stream |
| `body_consumed` | `BodyConsumeState::Untouched` | `BodyConsumeState::Untouched` | Initialized to same value |

### 15.4 Constraints

| ID | Rule |
|---|---|
| NRM-1 | Riverbed MUST use `fill_from_h1()` for all H1 requests. |
| NRM-2 | Riverbed MUST use `fill_from_h2()` for all H2 requests. |
| NRM-3 | Both functions MUST produce identical `Request` field layout. The dispatcher MUST NOT need to know which protocol carried the request. |
| NRM-4 | Invalid header names/values from H1 parsing MUST be silently dropped after structural validation passes. |
| NRM-5 | H1 `path` includes the query string (e.g., `/api/users?id=5`). The consumer parses query parameters. |
| NRM-6 | H2 `path` is extracted from `uri.path_and_query()`. Same format as H1. |

---

## 16. Hot Reload

Hot reload allows the consumer to swap the dispatcher at runtime without restarting the server. Riverbed provides the mechanism; the consumer decides when and what to reload.

### 16.1 Mechanism

Each worker spawns a background timer task at startup. The timer periodically checks the reload channel using non-blocking `try_recv()`.

```rust
// Internal — spawned on each worker's current-thread runtime.
async fn reload_check_loop<D: Dispatcher>(
    reload_rx: &mut mpsc::Receiver<D>,
    dispatcher: &RefCell<D>,
    interval: Duration,
) {
    let mut timer = tokio::time::interval(interval);
    loop {
        timer.tick().await;
        match reload_rx.try_recv() {
            Ok(new_dispatcher) => {
                dispatcher.replace(new_dispatcher);
                // Drain any additional pending dispatchers — only keep the latest.
                while let Ok(newer) = reload_rx.try_recv() {
                    dispatcher.replace(newer);
                }
            }
            Err(mpsc::error::TryRecvError::Empty) => continue,
            Err(mpsc::error::TryRecvError::Disconnected) => break,
        }
    }
}
```

### 16.2 Consumer Usage

```rust
// Consumer triggers reload:
let new_dispatcher = build_dispatcher_from_new_config();
server_handle.reload(new_dispatcher);
// Workers pick up the new dispatcher within reload_check_interval.
```

`ServerHandle::reload()` sends the new dispatcher to every worker's channel. Each worker picks it up on the next timer tick. In-flight requests on the old dispatcher complete naturally — the swap only affects new `dispatch()` calls after the swap.

### 16.3 Reload Latency

Worst-case reload latency is one `reload_check_interval`. With the recommended default of 250ms, a reload is applied within 250ms of the `ServerHandle::reload()` call.

### 16.4 Constraints

| ID | Rule |
|---|---|
| RLD-1 | Each worker MUST spawn a reload check timer task at startup. |
| RLD-2 | The timer interval MUST be `EngineFactory::reload_check_interval()`. |
| RLD-3 | Riverbed MUST use `try_recv()` (non-blocking). MUST NOT use `recv().await` in the timer loop. |
| RLD-4 | If multiple dispatchers are queued, riverbed MUST drain the channel and keep only the latest. |
| RLD-5 | The dispatcher MUST be stored in `RefCell<D>`. Swap is a `RefCell::replace()`. |
| RLD-6 | In-flight dispatch calls on the old dispatcher MUST complete normally. The swap affects only subsequent dispatches. |
| RLD-7 | `ServerHandle::reload()` MUST send to all worker channels. |

---

## 17. Graceful Shutdown

Graceful shutdown follows the same model as Rivers' existing `shutdown_signal` task. Workers stop accepting new connections and drain in-flight requests.

### 17.1 Shutdown Sequence

```
ServerHandle::shutdown() or shutdown_with_timeout(timeout) called
    │
    ▼
Broadcast shutdown signal via broadcast::Sender
    │
    ▼
Each worker receives signal in accept loop select! (§9.2)
    │
    ▼
Worker breaks out of accept loop — stops accepting new connections
    │
    ▼
Worker waits for all in-flight connection tasks to complete
    │
    ├── In-flight H1 requests complete normally
    ├── In-flight H2 streams complete normally
    ├── Streaming responses drain to completion
    └── Upgraded connections are left alone (consumer manages lifecycle)
    │
    ▼
Drain timeout expires → drop remaining in-flight tasks
    │
    ▼
Worker drops all pool objects
    │
    ▼
Worker's tokio runtime shuts down
    │
    ▼
Worker thread exits
    │
    ▼
ServerHandle::shutdown() joins all threads and returns
```

### 17.2 Constraints

| ID | Rule |
|---|---|
| SHT-1 | `ServerHandle::shutdown()` MUST broadcast a shutdown signal to all workers. |
| SHT-2 | Workers MUST stop accepting new connections immediately upon receiving the signal. |
| SHT-3 | Workers MUST wait for in-flight requests to complete, up to the drain timeout. |
| SHT-4 | Requests not completed within the drain timeout MUST be dropped. |
| SHT-5 | Workers MUST drop all pool objects before exiting. |
| SHT-6 | `ServerHandle::shutdown()` MUST join all worker threads and block until they exit. |
| SHT-7 | Default drain timeout is 30 seconds. Configurable via `shutdown_with_timeout()`. |

---

## 18. Multiple Instances

Riverbed supports multiple independent `Server` instances in a single process. Each instance has its own workers, its own listener address, its own TLS config, and its own dispatcher type.

### 18.1 Use Case

```rust
// Consumer code — e.g., riversd starting app traffic + RPS provisioning.
let app_handle = Server::start(
    "0.0.0.0:8443".parse().unwrap(),
    app_tls_config,                 // one-way TLS
    AppEngineFactory::new(config),
    num_cpus::get(),                // workers = CPU count
)?;

let rps_handle = Server::start(
    "0.0.0.0:9443".parse().unwrap(),
    rps_tls_config,                 // mTLS with client cert verification
    RpsEngineFactory::new(config),
    2,                              // low traffic, fewer workers
)?;

// Independent reload:
app_handle.reload(new_app_dispatcher);

// Independent shutdown:
rps_handle.shutdown();
app_handle.shutdown();
```

### 18.2 Constraints

| ID | Rule |
|---|---|
| MUL-1 | Each `Server::start()` call MUST produce fully independent worker threads. |
| MUL-2 | Instances MUST NOT share workers, pools, or channels. |
| MUL-3 | Each instance MUST have its own `ServerHandle` for independent shutdown and reload. |
| MUL-4 | Multiple instances MAY bind to different addresses or the same address with different ports. |

---

## 19. Standalone Mode

Riverbed ships with a `DefaultEngineFactory` for consumers that do not need custom factory implementations.

### 19.1 DefaultEngineFactory

```rust
pub struct DefaultEngineFactory<D: Dispatcher> {
    dispatcher: D,
    config: DefaultEngineConfig,
}

pub struct DefaultEngineConfig {
    pub body_limit: Option<usize>,          // default: None (unlimited)
    pub keep_alive_timeout: Duration,       // default: 75 seconds
    pub max_connections: Option<usize>,     // default: None (unlimited)
    pub idle_timeout: Duration,             // default: 60 seconds
    pub dispatch_timeout: Duration,         // default: 30 seconds
    pub pool_capacity: usize,              // default: 128
    pub stream_channel_capacity: usize,    // default: 32
    pub reload_check_interval: Duration,   // default: 250ms
}

impl Default for DefaultEngineConfig {
    fn default() -> Self {
        DefaultEngineConfig {
            body_limit: None,
            keep_alive_timeout: Duration::from_secs(75),
            max_connections: None,
            idle_timeout: Duration::from_secs(60),
            dispatch_timeout: Duration::from_secs(30),
            pool_capacity: 128,
            stream_channel_capacity: 32,
            reload_check_interval: Duration::from_millis(250),
        }
    }
}

impl<D: Dispatcher> DefaultEngineFactory<D> {
    pub fn new(dispatcher: D, config: DefaultEngineConfig) -> Self {
        DefaultEngineFactory { dispatcher, config }
    }
}

impl<D: Dispatcher> EngineFactory<D> for DefaultEngineFactory<D> {
    fn dispatcher(&self) -> D { self.dispatcher.clone() }
    fn body_limit(&self) -> Option<usize> { self.config.body_limit }
    fn keep_alive_timeout(&self) -> Duration { self.config.keep_alive_timeout }
    fn max_connections(&self) -> Option<usize> { self.config.max_connections }
    fn idle_timeout(&self) -> Duration { self.config.idle_timeout }
    fn dispatch_timeout(&self) -> Duration { self.config.dispatch_timeout }
    fn pool_capacity(&self) -> usize { self.config.pool_capacity }
    fn stream_channel_capacity(&self) -> usize { self.config.stream_channel_capacity }
    fn reload_check_interval(&self) -> Duration { self.config.reload_check_interval }
}
```

### 19.2 Minimal Consumer Example

```rust
use riverbed_httpd::{
    Server, Dispatcher, Request, Response, EngineError,
    DefaultEngineFactory, DefaultEngineConfig,
};
use bytes::Bytes;
use std::sync::Arc;

#[derive(Clone)]
struct HelloWorld;

impl Dispatcher for HelloWorld {
    async fn dispatch(
        &self,
        _req: &Request,
        res: &mut Response,
    ) -> Result<(), EngineError> {
        res.set_status(200);
        res.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::header::HeaderValue::from_static("text/plain"),
        );
        res.set_body_fixed(Bytes::from_static(b"Hello, world!"));
        Ok(())
    }
}

fn main() -> std::io::Result<()> {
    let tls_config = build_tls_config(); // consumer builds this

    let factory = DefaultEngineFactory::new(
        HelloWorld,
        DefaultEngineConfig::default(),
    );

    let handle = Server::start(
        "0.0.0.0:8443".parse().unwrap(),
        Arc::new(tls_config),
        factory,
        num_cpus::get(),
    )?;

    // Block until SIGTERM/SIGINT
    wait_for_signal();
    handle.shutdown();
    Ok(())
}
```

### 19.3 Constraints

| ID | Rule |
|---|---|
| STD-1 | `DefaultEngineFactory` MUST implement `EngineFactory` with configurable defaults. |
| STD-2 | `DefaultEngineConfig::default()` MUST provide sane production defaults as documented. |
| STD-3 | Standalone consumers MUST only need to implement `Dispatcher`. `EngineFactory` is optional via `DefaultEngineFactory`. |

---

## 20. Validation Rules

Riverbed validates structural HTTP protocol correctness. Application-level validation (method names, path semantics, auth, routing) is the consumer's responsibility.

### 20.1 Structural Validation (riverbed MUST enforce)

| ID | Check | HTTP Status | Action |
|---|---|---|---|
| VAL-1 | Null bytes (`\0`) in request line or headers | 400 | Close connection |
| VAL-2 | Request line exceeds 8,192 bytes | 414 | Close connection |
| VAL-3 | Single header value exceeds 8,192 bytes | 431 | Close connection |
| VAL-4 | Total header block exceeds 64 KiB | 431 | Close connection |
| VAL-5 | More than 128 headers | 431 | Close connection |
| VAL-6 | Malformed request line (missing method, path, or HTTP version) | 400 | Close connection |
| VAL-7 | Content-Length present but not a valid unsigned integer | 400 | Close connection |
| VAL-8 | Both `Content-Length` and `Transfer-Encoding: chunked` present | 400 | Close connection |
| VAL-9 | Chunked encoding: malformed chunk size or missing terminator | 400 | Close connection |
| VAL-10 | HTTP version is not `HTTP/1.0` or `HTTP/1.1` (H1 only) | 505 | Close connection |

### 20.2 Not Validated by Riverbed (consumer MUST handle if needed)

| Concern | Reason |
|---|---|
| Method names (GET, POST, PURGE, etc.) | Application-level. Consumer decides valid methods. |
| Path format and semantics | Application-level. Consumer decides valid paths. |
| Query string parsing | Application-level. Riverbed passes raw path including query. |
| Content-Type interpretation | Application-level. |
| Request body schema validation | Application-level. |
| Authentication and authorization | Application-level. |
| CORS | Application-level. |
| Rate limiting | Application-level. |
| Body size limits | Consumer-configured via EngineFactory, enforced by riverbed at read time. |

### 20.3 H2 Validation

H2 protocol compliance is handled by the `h2` crate. Riverbed MUST NOT reimplement H2 validation. Protocol errors from `h2` result in connection reset per HTTP/2 spec (GOAWAY or RST_STREAM frames).

### 20.4 Constraints

| ID | Rule |
|---|---|
| VAL-11 | Riverbed MUST validate all items in §20.1 before calling `fill_from_h1()`. |
| VAL-12 | Riverbed MUST NOT validate items listed in §20.2. |
| VAL-13 | H2 protocol errors MUST be handled by the `h2` crate's built-in error mechanism. Riverbed MUST NOT reimplement H2 validation. |
| VAL-14 | All structural validation failures MUST close the connection. No keep-alive after a protocol violation. |
