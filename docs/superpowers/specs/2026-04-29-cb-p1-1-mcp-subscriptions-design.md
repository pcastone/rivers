# CB P1.1 Design: MCP Resource Subscriptions / Push Notifications

**Date:** 2026-04-29
**Status:** Draft — for review

---

## Scope

Implement the MCP spec's resource subscription model in Rivers' MCP server: clients call `resources/subscribe` on a URI and receive `notifications/resources/updated` pushed from the server when the underlying data changes. Closes CB feature request P1.1 (`docs/bugs/cb-rivers-feature-request.md:90`).

Out of scope for v1:
- Per-driver native change feeds (Postgres LISTEN/NOTIFY, Mongo change streams, Kafka). v1 uses polling only.
- Subscription persistence across reconnects. Subscriptions live with the SSE connection; if the client reconnects it must re-subscribe.
- `notifications/resources/list_changed` (resource catalog mutations). Rivers' resource catalog is static per bundle, so this notification is moot until hot-reload exists.

---

## Problem

CB has event streams that fit the subscription model — Ape Flag firings, investigation queue updates, approval queue updates, sprint contract drift. Today CB polls these via repeated `resources/read` calls, burning conversation turns and adding latency. Rivers' current MCP implementation has no subscribe/notify path:

- Method dispatcher (`crates/riversd/src/mcp/dispatch.rs:35-46`) only handles `initialize`, `ping`, `tools/{list,call}`, `resources/{list,read,templates/list}`, `prompts/{list,get}`. Anything else returns `method_not_found`.
- `initialize` capabilities (`dispatch.rs:59-67`) advertise no `subscribe` flag on resources.
- Transport (`crates/riversd/src/server/view_dispatch.rs:468-510`, `execute_mcp_view`) is strictly request/response: read POST body up to 16 MiB, dispatch, return JSON. There is no streaming path on which the server could send a notification.
- `McpResourceConfig` (`crates/rivers-runtime/src/view.rs:412`) has no subscription metadata.

Every existing piece — sessions, URI templates, the DataView executor, the EventBus — works the way subscriptions need them to. The missing pieces are the streaming transport, the subscription registry, and the change source.

---

## Design

Three layers, each independently testable.

### Layer 1 — Streamable HTTP transport

The MCP "Streamable HTTP" transport spec allows a single endpoint to either return JSON for one-shot requests or upgrade to SSE for long-lived sessions where the server pushes notifications. Rivers' MCP endpoint currently does the former only. v1 adds the latter when the client opts in via `Accept: text/event-stream`.

**Behavior:**

In `execute_mcp_view`:

1. If `Accept` header includes `text/event-stream` AND a valid `Mcp-Session-Id` is present AND the request is a `GET` (per MCP spec — clients open the SSE channel with a separate GET to the MCP endpoint after `initialize` over POST):
   - Return an `axum::response::sse::Sse` response.
   - Register the SSE sender with the session's `NotificationChannel` (see Layer 2).
   - Hold the connection open. On client disconnect, drop the channel and clear that session's subscriptions.
2. Otherwise (POST with JSON or POST with `Accept: application/json`): existing one-shot path, unchanged.

POST requests that produce a notification-bearing response (`resources/subscribe` ack) still return JSON over the POST. Notifications themselves only flow over the SSE channel.

**Initialize capability change:**

```json
"capabilities": {
  "tools": { "listChanged": false },
  "resources": { "subscribe": true }
}
```

The `subscribe: true` flag is added when the bundle declares any resource with `subscribable = true` (see Layer 4).

### Layer 2 — Subscription registry & notification channel

A new module `crates/riversd/src/mcp/subscriptions.rs` exposes:

```rust
/// Per-session subscription state. In-memory only — survives the lifetime
/// of the SSE connection, not session-id reuse across reconnects.
pub struct SubscriptionRegistry {
    /// session_id → (sse_sender, set of subscribed URIs)
    sessions: tokio::sync::RwLock<HashMap<String, SessionChannel>>,
}

pub struct SessionChannel {
    pub sender: tokio::sync::mpsc::Sender<sse::Event>,
    pub subscribed_uris: HashSet<String>,
    pub app_id: String,
}

impl SubscriptionRegistry {
    pub async fn attach_sse(&self, session_id: &str, app_id: &str) -> mpsc::Receiver<sse::Event>;
    pub async fn detach(&self, session_id: &str);
    pub async fn subscribe(&self, session_id: &str, uri: &str) -> Result<(), SubscribeError>;
    pub async fn unsubscribe(&self, session_id: &str, uri: &str);
    pub async fn notify_changed(&self, app_id: &str, uri: &str);
    pub async fn snapshot_subscriptions(&self) -> Vec<(String /*app_id*/, String /*uri*/)>;
}
```

`SubscriptionRegistry` is owned by `AppContext` (single instance per `riversd`). The mpsc channel is bounded (capacity 64) — if a slow client backs up, `try_send` drops the notification and emits a `WARN` log. Notifications are coalesced opportunistically: if the same URI is already pending in the channel, the new notification is skipped (cheap dedupe by URI lookup before send).

**Bounded subscription count:** Reject `resources/subscribe` with JSON-RPC error `-32000` ("Too many subscriptions") when a single session exceeds `max_subscriptions_per_session` (default 100, configurable).

### Layer 3 — Change source (polling, v1)

A background task per `(app_id, uri)` poll-tuple runs while ≥1 session subscribes. New module `crates/riversd/src/mcp/poller.rs`:

```rust
pub struct ChangePoller {
    registry: Arc<SubscriptionRegistry>,
    dataview_executor: Arc<RwLock<Option<Arc<DataViewExecutor>>>>,
    /// (app_id, uri) → join handle
    handles: tokio::sync::Mutex<HashMap<(String, String), JoinHandle<()>>>,
}
```

On each `subscribe`:

1. If a poller for `(app_id, uri)` already exists, just add the session to the registry — the poller's notifications fan out to all subscribers via `notify_changed`.
2. Otherwise spawn a new task that:
   - Resolves the URI to its DataView (re-using the resolution logic from `handle_resources_read`).
   - Executes the DataView with the URI's template variables.
   - Hashes `query_result.rows` (BLAKE3, fast and stable).
   - Sleeps `poll_interval` (default 5s, configurable per resource).
   - Re-executes; if hash differs, calls `registry.notify_changed(app_id, uri)`.
   - Exits when the registry reports zero subscribers for `(app_id, uri)`.

**Refcount cleanup:** `unsubscribe` and connection-drop both decrement; reaching zero stops the poller.

### Layer 4 — Config surface

Extend `McpResourceConfig` (`crates/rivers-runtime/src/view.rs:412`):

```rust
pub struct McpResourceConfig {
    pub dataview: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_mime")]
    pub mime_type: String,
    #[serde(default)]
    pub uri_template: Option<String>,

    /// When true, this resource accepts `resources/subscribe` requests and
    /// emits `notifications/resources/updated` when underlying data changes.
    #[serde(default)]
    pub subscribable: bool,

    /// Polling interval in seconds for change detection. Default: 5.
    /// Ignored when `subscribable = false`.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_seconds: u64,
}
```

Plus a global ceiling in `[mcp]` config section:

```toml
[mcp]
max_subscriptions_per_session = 100   # default
min_poll_interval_seconds = 1         # safety floor (default)
```

`min_poll_interval_seconds` clamps `poll_interval_seconds` from below — prevents a hostile bundle from spinning DataView execution.

### Layer 5 — JSON-RPC method handlers

Add to `dispatch.rs:35-46`:

```rust
"resources/subscribe"   => handle_resources_subscribe(req, resources, registry, session_id, app_id).await,
"resources/unsubscribe" => handle_resources_unsubscribe(req, registry, session_id).await,
```

Both require `Mcp-Session-Id`. Both validate the URI matches a subscribable resource (`subscribable = true`). `subscribe` triggers `ChangePoller::ensure_running((app_id, uri))`.

Notification frame (sent over SSE):

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/resources/updated",
  "params": { "uri": "cb://proj-a/decisions" }
}
```

The session_id reaches `dispatch` via a new parameter — currently `dispatch` does not receive it, but `view_dispatch.rs:514-520` already extracts it before calling. Plumb it through.

---

## Files

| File | Change |
|------|--------|
| `crates/rivers-runtime/src/view.rs` | Add `subscribable`, `poll_interval_seconds` to `McpResourceConfig` |
| `crates/rivers-core-config/src/config/mcp.rs` | New — `McpConfig { max_subscriptions_per_session, min_poll_interval_seconds }` |
| `crates/rivers-core-config/src/config/mod.rs` | Export `McpConfig` |
| `crates/rivers-core-config/src/config/runtime.rs` | Add `mcp: Option<McpConfig>` to `ServerConfig` |
| `crates/riversd/src/mcp/subscriptions.rs` | New — `SubscriptionRegistry`, `SessionChannel` |
| `crates/riversd/src/mcp/poller.rs` | New — `ChangePoller`, hash-diff loop |
| `crates/riversd/src/mcp/dispatch.rs` | Add `resources/subscribe`, `resources/unsubscribe`; advertise `resources.subscribe = true` in `initialize` when any resource is subscribable; thread session_id through |
| `crates/riversd/src/mcp/mod.rs` | Re-export new modules |
| `crates/riversd/src/server/view_dispatch.rs` | SSE branch in `execute_mcp_view` for `Accept: text/event-stream` GET; attach SSE channel to registry; detach on disconnect |
| `crates/riversd/src/server/lifecycle.rs` | Construct `SubscriptionRegistry` and `ChangePoller` at startup; place on `AppContext` |
| `crates/rivers-runtime/src/validate_structural.rs` | New rule `S-MCP-2`: warn when `subscribable = true` but resource's DataView has no GET method (subscriptions need a readable view) |

---

## Risks & open questions

**Risk: notification storms.** A bundle with 50 subscribable resources and 100 sessions each subscribing to all 50 = 5000 pollers. Mitigated by per-(app_id, uri) sharing — actual pollers cap at ≤ resource count. Still, a 1-second poll interval × 50 resources × heavy DataViews could swamp the executor. The `min_poll_interval_seconds` floor is the lever; default of 1 may be too aggressive.

**Risk: hash false-positives.** Two reads in a row may differ by row order even when set-equivalent (e.g. unstable Postgres ORDER missing). v1 hashes verbatim — bundle authors must provide deterministic ordering. Document this loudly in the resource subscription guide.

**Open: should a fresh subscriber receive an immediate snapshot?** MCP spec is silent. Plan: no. Subscription only sends *deltas*. Clients call `resources/read` first, then `resources/subscribe`. Document the pattern.

**Open: SSE keepalive.** Default to 30-second comment-only keepalive frames to keep proxies happy. Reuse Rivers' existing SSE driver code if present, otherwise hand-roll on `axum::response::sse`.

---

## Validation

1. Bundle with `subscribable = true` resource. Open SSE channel; subscribe; mutate underlying DataView; observe notification within `poll_interval_seconds + jitter`.
2. Two sessions subscribe to the same URI; both receive notifications; only one poller runs (verified via debug log).
3. Session disconnects mid-stream; poller refcount drops to 0; poller task exits within one poll cycle.
4. Subscribe to 101 URIs on one session with `max_subscriptions_per_session = 100`; the 101st returns JSON-RPC error -32000.
5. `subscribable = false` resource: `resources/subscribe` returns JSON-RPC error.
6. Notification storm: slow client (capacity-full mpsc); confirm `try_send` drops notification, WARN logged, no panic, no memory growth.
7. `initialize` capabilities advertise `resources.subscribe = true` only when ≥1 resource has `subscribable = true`.

---

## Implementation order

1. Layer 4 (config surface) — additive, shippable alone.
2. Layer 2 (registry) — testable via unit tests with fake mpsc receivers.
3. Layer 1 (SSE transport) — wire `axum::response::sse` into `execute_mcp_view`.
4. Layer 5 (subscribe/unsubscribe handlers) — round-trip works without a poller (nothing to notify yet).
5. Layer 3 (poller) — completes the feature.

## Version bump

`bump-minor` — this is genuinely new ground (streaming MCP transport + change-detection subsystem), not a gap-fill.
