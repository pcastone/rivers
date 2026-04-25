# Riverbed HTTPD — Build Plan

**Spec:** `docs/arch/riverbed-httpd-spec.md`
**Status:** Future consideration
**Crate:** `riverbed-httpd` (new workspace member)

---

## Phase 1: Core Engine (crate: `riverbed-httpd`)

### 1.1 Server & Worker Model (spec §3–4)
- [ ] `Server::start()` — spawn N OS threads, each with own `current_thread` tokio runtime
- [ ] `SO_REUSEPORT` TCP listener binding per worker
- [ ] `WorkerConfig` struct with all tunable parameters
- [ ] Supervisor thread — holds reload/shutdown senders, joins workers on shutdown

**Validate:**
- [ ] Verify N workers spawn and each binds its own listener (log port + thread ID)
- [ ] Verify workers are fully independent — no shared mutable state
- [ ] Stress test: start/stop 100 times, no leaked threads or dangling listeners

### 1.2 TLS & Protocol Negotiation (spec §5)
- [ ] TLS accept via `tokio-rustls` with consumer-provided `ServerConfig`
- [ ] ALPN negotiation — route to H1 or H2 branch
- [ ] mTLS support (client cert verification)

**Validate:**
- [ ] H1 client connects and gets response over TLS
- [ ] H2 client connects via ALPN and gets response over TLS
- [ ] Plain HTTP request rejected (TLS-only)
- [ ] Invalid TLS handshake does not crash worker

### 1.3 Request & Response Types (spec §6–7)
- [ ] `Request` type with lazy body reading (`BodyReader` enum: H1/H2/Empty)
- [ ] `Response` type with `ResponseBody` enum (Fixed/Stream/Upgrade)
- [ ] `BodyConsumeState` tracking (Untouched/Partial/Consumed)
- [ ] Reset methods for pool recycling

**Validate:**
- [ ] Request body read lazily — headers available before body is consumed
- [ ] Large body streamed without full buffering
- [ ] Response with Fixed body writes correctly
- [ ] Response with Stream body delivers chunks in order
- [ ] Reset clears all fields, object is reusable

### 1.4 Object Pooling (spec §8)
- [ ] `ObjectPool<T>` with checkout/checkin lifecycle
- [ ] `Poolable` trait (`new_default`, `reset`)
- [ ] Implement `Poolable` for `Request` and `Response`

**Validate:**
- [ ] Pool recycles objects — no allocation after warmup under steady load
- [ ] Pool respects capacity limit — excess objects dropped, not queued
- [ ] Recycled request/response has no data from previous use

### 1.5 Worker Loop State Machine (spec §9)
- [ ] Connection accept loop with `select!` (accept, reload, shutdown)
- [ ] Per-request state machine for H1 (read → dispatch → write → drain → loop/close)
- [ ] Per-stream state machine for H2 (accept stream → dispatch → send response)

**Validate:**
- [ ] H1 keep-alive — multiple requests on one connection
- [ ] H1 connection closed after idle timeout
- [ ] H2 concurrent streams on one connection
- [ ] Slow client does not block other connections

### 1.6 Dispatcher & EngineFactory Traits (spec §10–11)
- [ ] `Dispatcher` trait — `async fn dispatch(&self, req: &mut Request, res: &mut Response)`
- [ ] `EngineFactory` trait — config policies (body limit, timeouts, pool capacity)
- [ ] Clone semantics for dispatcher (one clone per worker)

**Validate:**
- [ ] Custom dispatcher receives requests and can set response status/body
- [ ] EngineFactory config values are respected (body limit enforced, timeouts fire)
- [ ] Dispatcher panic does not crash worker (caught via `catch_unwind`)

### 1.7 H1/H2 Normalization (spec §15)
- [ ] `fill_from_h1` — parse raw HTTP/1.1 into `Request`
- [ ] `fill_from_h2` — map `h2::RecvStream` into `Request`
- [ ] Field mapping table (method, path, headers normalized across protocols)

**Validate:**
- [ ] Same dispatcher code works for both H1 and H2 requests
- [ ] Headers normalized consistently (lowercase, no pseudo-headers leaked)
- [ ] Malformed H1 request line returns 400, does not crash

### 1.8 Body Handling (spec §13–14)
- [ ] Lazy body reading with configurable body limit
- [ ] Unconsumed body drain with timeout
- [ ] Response drain loops — Fixed body, Stream body, Upgrade
- [ ] Body limit exceeded returns 413

**Validate:**
- [ ] Body over limit returns 413 and closes connection
- [ ] Unconsumed request body drained before next request on keep-alive
- [ ] Drain timeout fires — connection closed, no hang
- [ ] Stream response completes even if client reads slowly

### 1.9 Error Handling (spec §12)
- [ ] `EngineError` enum (BodyReadFailed, BodyLimitExceeded, StreamWriteFailed, DispatchPanicked, Timeout, Custom)
- [ ] `CustomError` with status, close_connection flag, optional body
- [ ] Worker error response behavior — minimal fixed responses for engine errors

**Validate:**
- [ ] Each `EngineError` variant produces correct HTTP status code
- [ ] `CustomError` with `close_connection = true` closes after response
- [ ] Dispatcher panic produces 500, connection stays open for next request

### 1.10 Hot Reload (spec §16)
- [ ] Supervisor broadcasts new `Dispatcher` to workers via mpsc
- [ ] Workers swap dispatcher on reload timer tick
- [ ] In-flight requests on old dispatcher complete naturally

**Validate:**
- [ ] Reload under load — no dropped requests, no 500s
- [ ] New dispatcher takes effect within `reload_check_interval`
- [ ] Old dispatcher dropped after all in-flight requests complete

### 1.11 Graceful Shutdown (spec §17)
- [ ] `ServerHandle::shutdown()` broadcasts to all workers
- [ ] Workers stop accepting new connections, drain in-flight requests
- [ ] `shutdown_with_timeout()` — force-close after timeout
- [ ] `ServerHandle::shutdown()` blocks until all workers exit

**Validate:**
- [ ] In-flight request completes during shutdown drain
- [ ] No new connections accepted after shutdown signal
- [ ] Workers exit within timeout — no hung threads
- [ ] After shutdown, port is released (can rebind immediately)

### 1.12 Standalone Mode (spec §19)
- [ ] `DefaultEngineFactory` with `DefaultEngineConfig` defaults
- [ ] Minimal consumer example compiles and runs (HelloWorld dispatcher)

**Validate:**
- [ ] HelloWorld example starts, responds to requests, shuts down cleanly
- [ ] All `DefaultEngineConfig` defaults match spec values

### 1.13 Validation Rules (spec §20)
- [ ] Null bytes in headers rejected
- [ ] Malformed request lines rejected
- [ ] Header size overflow rejected
- [ ] Content-length integrity enforced
- [ ] Chunked encoding framing validated

**Validate:**
- [ ] Each validation rule has a dedicated test with crafted malformed input
- [ ] Rejected requests return appropriate 4xx status
- [ ] No crash or panic on any malformed input

---

## Phase 2: Integration with riversd

### 2.1 Dispatcher Bridge
- [ ] Implement `Dispatcher` in riversd — bridge to existing view engine pipeline
- [ ] Map Riverbed `Request` → view engine context
- [ ] Map view engine result → Riverbed `Response`

**Validate:**
- [ ] Existing bundle endpoints return same responses as Axum stack
- [ ] Request headers, path, query params, body all forwarded correctly

### 2.2 EngineFactory Bridge
- [ ] Implement `EngineFactory` in riversd — map `ServerConfig` to engine policies
- [ ] Body limit, timeouts, worker count sourced from `riversd.toml`

**Validate:**
- [ ] Config changes in `riversd.toml` reflected in engine behavior

### 2.3 Routing
- [ ] Replace Axum router with direct route matching (reuse `matchit` or custom)
- [ ] Path parameter extraction without Axum extractors

**Validate:**
- [ ] All existing routes resolve identically
- [ ] Path parameters extracted correctly (`:id`, wildcards)
- [ ] 404 for unknown routes

### 2.4 Middleware Pipeline
- [ ] Convert Tower middleware to explicit dispatch pipeline
- [ ] CORS handling
- [ ] Authentication / session validation
- [ ] Rate limiting
- [ ] Compression

**Validate:**
- [ ] CORS preflight returns correct headers
- [ ] Auth rejection returns 401/403
- [ ] Rate limiting triggers at configured threshold
- [ ] Compressed responses decode correctly

### 2.5 Streaming Protocols
- [ ] WebSocket upgrade via `ResponseBody::Upgrade`
- [ ] SSE via `ResponseBody::Stream`

**Validate:**
- [ ] WebSocket connect, send, receive, close lifecycle works
- [ ] SSE stream delivers events, client reconnect works

### 2.6 Admin API
- [ ] Admin API as second `Server` instance (spec §18)
- [ ] All admin endpoints functional

**Validate:**
- [ ] Admin API responds on separate port
- [ ] Admin auth still enforced

### 2.7 Static Files
- [ ] Static file serving without Axum extractors
- [ ] SPA fallback routing

**Validate:**
- [ ] Static files served with correct content-type
- [ ] SPA fallback returns index.html for unknown paths

### 2.8 Dependency Removal
- [ ] Remove `axum`, `tower`, `tower-http`, `hyper`, `hyper-util` from workspace

**Validate:**
- [ ] `cargo build` succeeds with no Axum/Tower/Hyper references
- [ ] Binary size comparison (expect smaller)

---

## Phase 3: System Validation

- [ ] Full canary suite (69/69 tests) passes on Riverbed engine
- [ ] Load test: compare throughput/latency vs Axum stack (wrk or k6)
- [ ] Hot reload under load — swap bundle, verify zero dropped requests
- [ ] Graceful shutdown under load — verify in-flight requests complete
- [ ] Memory profile — verify object pooling reduces allocation pressure vs Axum
- [ ] Fuzz testing — malformed HTTP input does not crash or hang
- [ ] All existing integration tests pass without modification
