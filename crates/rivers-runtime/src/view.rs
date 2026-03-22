//! API View configuration types.
//!
//! Per `rivers-view-layer-spec.md` §12 and `rivers-technology-path-spec.md`.

use std::collections::HashMap;

use schemars::JsonSchema;
use serde::Deserialize;

/// Configuration for a single API view (REST, WebSocket, SSE, MessageConsumer).
///
/// Declared in `app.toml` under `[api.views.{view_id}]`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ApiViewConfig {
    /// "Rest" | "Websocket" | "ServerSentEvents" | "MessageConsumer"
    pub view_type: String,

    /// URL path pattern, e.g. "/api/contacts/:id"
    pub path: Option<String>,

    /// HTTP method: "GET", "POST", "PUT", "DELETE", etc.
    pub method: Option<String>,

    /// Handler definition — dataview or codecomponent.
    pub handler: HandlerConfig,

    /// Parameter mapping from HTTP params to DataView params.
    #[serde(default)]
    pub parameter_mapping: Option<ParameterMappingConfig>,

    // ── DataView pre-fetch & streaming ────────────────────────────────

    /// Pre-fetched DataViews resolved before the handler chain runs.
    #[serde(default)]
    pub dataviews: Vec<String>,

    /// Which DataView populates `ctx.resdata` (must be listed in `dataviews`).
    pub primary: Option<String>,

    /// When true, the primary DataView is dispatched in streaming mode.
    pub streaming: Option<bool>,

    /// Streaming serialization format: `"ndjson"` or `"sse"`.
    pub streaming_format: Option<String>,

    /// Streaming inactivity timeout in milliseconds.
    pub stream_timeout_ms: Option<u64>,

    // ── Auth ─────────────────────────────────────────────────────────

    /// If true, this view is the guard (auth entry point). Only one per server.
    #[serde(default)]
    pub guard: bool,

    /// Auth mode: "session" (default, protected), "none" (public).
    pub auth: Option<String>,

    /// Guard config — only valid when guard=true.
    #[serde(default)]
    pub guard_config: Option<GuardConfig>,

    // ── Outbound HTTP capability ────────────────────────────────────

    /// When true, this view's CodeComponent has access to `Rivers.http`.
    /// Per spec §10.5 — only views with explicit opt-in get outbound HTTP.
    #[serde(default)]
    pub allow_outbound_http: bool,

    // ── Rate limiting (per-view override) ───────────────────────────

    pub rate_limit_per_minute: Option<u32>,
    pub rate_limit_burst_size: Option<u32>,

    // ── View-type-specific fields ───────────────────────────────────

    /// WebSocket mode: "Broadcast" | "Direct" (WebSocket only).
    pub websocket_mode: Option<String>,

    /// Max concurrent WebSocket connections.
    pub max_connections: Option<usize>,

    /// SSE tick interval in milliseconds.
    pub sse_tick_interval_ms: Option<u64>,

    /// Event names that trigger SSE pushes.
    #[serde(default)]
    pub sse_trigger_events: Vec<String>,

    /// Max events retained in SSE replay buffer for Last-Event-ID reconnection.
    /// Defaults to 100 if not specified.
    #[serde(default)]
    pub sse_event_buffer_size: Option<usize>,

    /// Session revalidation interval (WebSocket / SSE).
    pub session_revalidation_interval_s: Option<u64>,

    /// Polling configuration (SSE/WS only) — tick-based DataView execution with diff.
    pub polling: Option<PollingConfig>,

    // ── Event handlers (pipeline stages) ────────────────────────────

    #[serde(default)]
    pub event_handlers: Option<ViewEventHandlers>,

    /// Stream handler for WebSocket views.
    pub on_stream: Option<OnStreamConfig>,

    /// WebSocket lifecycle hooks per technology-path-spec §14.2.
    pub ws_hooks: Option<WebSocketHooks>,

    /// Event handler for MessageConsumer views.
    pub on_event: Option<OnEventConfig>,
}

/// Guard view configuration.
///
/// Per `rivers-auth-session-spec.md` §3.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct GuardConfig {
    /// URL to redirect to on valid session (already logged in).
    pub valid_session_url: Option<String>,

    /// URL to redirect to on invalid session.
    pub invalid_session_url: Option<String>,

    /// Include session token in guard response body for API/mobile clients.
    #[serde(default)]
    pub include_token_in_body: bool,

    /// Key name for token in response body (default: "token").
    #[serde(default = "default_token_body_key")]
    pub token_body_key: String,
}

fn default_token_body_key() -> String {
    "token".to_string()
}

/// Handler definition — either a DataView reference, a CodeComponent, or none.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HandlerConfig {
    /// Handler that dispatches to a named DataView.
    Dataview {
        dataview: String,
    },

    /// Handler that runs a WASM CodeComponent.
    Codecomponent {
        language: String,
        module: String,
        entrypoint: String,
        #[serde(default)]
        resources: Vec<String>,
    },

    /// Null handler — no primary datasource. Used for views that only
    /// run CodeComponent pipeline stages (pre_process, handlers, etc.)
    /// with `datasource = "none"`.
    None {},
}

/// Parameter mapping from HTTP request to DataView parameters.
///
/// Per spec: uses `[api.views.*.parameter_mapping.query]`, `.path`, and `.body` subtables.
/// Format: `{http_param} = "{dataview_param}"`.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ParameterMappingConfig {
    #[serde(default)]
    pub query: HashMap<String, String>,

    #[serde(default)]
    pub path: HashMap<String, String>,

    /// Body parameter mapping for write operations (POST/PUT/PATCH).
    #[serde(default)]
    pub body: HashMap<String, String>,
}

/// Pipeline event handlers for a view.
///
/// Collapsed 4-stage pipeline per `rivers-technology-path-spec.md`:
///   pre_process → handlers → post_process + on_error
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ViewEventHandlers {
    #[serde(default)]
    pub pre_process: Vec<HandlerStageConfig>,

    /// Ordered handler chain — replaces the former transform/on_response stages.
    #[serde(default)]
    pub handlers: Vec<HandlerStageConfig>,

    #[serde(default)]
    pub post_process: Vec<HandlerStageConfig>,

    #[serde(default)]
    pub on_error: Vec<HandlerStageConfig>,
}

/// A single pipeline stage handler reference.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HandlerStageConfig {
    pub module: String,
    pub entrypoint: String,
    pub key: Option<String>,
    pub on_failure: Option<String>,
}

/// `[api.views.*.on_stream]` — WebSocket stream handler.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OnStreamConfig {
    pub module: String,
    pub entrypoint: String,
    /// "Stream" | "Normal" | "Auto"
    pub handler_mode: Option<String>,
}

/// WebSocket lifecycle hooks per technology-path-spec §14.2.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WebSocketHooks {
    pub on_connect: Option<HandlerStageConfig>,
    pub on_message: Option<HandlerStageConfig>,
    pub on_disconnect: Option<HandlerStageConfig>,
}

/// `[api.views.*.on_event]` — MessageConsumer event handler.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OnEventConfig {
    pub topic: String,
    pub handler: String,
    pub handler_mode: Option<String>,
}

/// Polling configuration for SSE/WS views.
///
/// When present on an SSE or WebSocket view, the framework manages a poll loop
/// that periodically executes a DataView, diffs results, and pushes changes.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PollingConfig {
    /// Tick interval in milliseconds (must be > 0).
    pub tick_interval_ms: u64,

    /// Diff strategy: "hash" (default), "null", or "change_detect".
    #[serde(default = "default_diff_strategy")]
    pub diff_strategy: String,

    /// TTL for persisted poll state in seconds (0 = no expiry).
    #[serde(default)]
    pub poll_state_ttl_s: u64,

    /// CodeComponent to invoke when data changes.
    pub on_change: Option<OnChangeConfig>,

    /// CodeComponent for custom diff logic (when diff_strategy = "change_detect").
    pub change_detect: Option<ChangeDetectConfig>,
}

fn default_diff_strategy() -> String {
    "hash".to_string()
}

/// `on_change` handler — invoked when polled data changes.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OnChangeConfig {
    pub module: String,
    pub entrypoint: String,
}

/// `change_detect` handler — custom diff logic via CodeComponent.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ChangeDetectConfig {
    pub module: String,
    pub entrypoint: String,
}
