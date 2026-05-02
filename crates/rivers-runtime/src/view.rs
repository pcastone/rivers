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

    /// Per-view rate limit: max requests per minute.
    pub rate_limit_per_minute: Option<u32>,
    /// Per-view rate limit: burst allowance above the sustained rate.
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

    /// Pipeline event handlers (pre_process, handlers, post_process, on_error).
    #[serde(default)]
    pub event_handlers: Option<ViewEventHandlers>,

    /// Stream handler for WebSocket views.
    pub on_stream: Option<OnStreamConfig>,

    /// WebSocket lifecycle hooks per technology-path-spec §14.2.
    pub ws_hooks: Option<WebSocketHooks>,

    /// Event handler for MessageConsumer views.
    pub on_event: Option<OnEventConfig>,

    // ── MCP fields ───────────────────────────────────────────────────

    /// MCP tool declarations — whitelisted DataViews exposed as MCP tools.
    #[serde(default)]
    pub tools: HashMap<String, McpToolConfig>,

    /// MCP resource declarations — read-only DataViews exposed as MCP resources.
    #[serde(default)]
    pub resources: HashMap<String, McpResourceConfig>,

    /// MCP prompt declarations — markdown templates for AI workflows.
    #[serde(default)]
    pub prompts: HashMap<String, McpPromptConfig>,

    /// Path to static instructions markdown file (relative to app root).
    #[serde(default)]
    pub instructions: Option<String>,

    /// MCP session configuration.
    #[serde(default)]
    pub session: Option<McpSessionConfig>,

    /// MCP federation upstreams — remote MCP servers whose tools/resources are merged
    /// into this server's `tools/list` and `resources/list` under a namespaced prefix.
    #[serde(default)]
    pub federation: Vec<McpFederationConfig>,
}

/// Guard lifecycle hooks — all optional, all side-effects only.
///
/// Per technology-path-spec §9.5: hooks cannot influence auth flow.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct GuardLifecycleHooks {
    /// Fires when session already exists and is valid.
    pub on_session_valid: Option<HandlerStageConfig>,

    /// Fires on invalid or expired session.
    pub on_invalid_session: Option<HandlerStageConfig>,

    /// Fires on credential validation failure.
    pub on_failed: Option<HandlerStageConfig>,
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

    /// Guard lifecycle hooks — side-effect-only callbacks.
    #[serde(default)]
    pub lifecycle_hooks: Option<GuardLifecycleHooks>,
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
        /// DataView name (must exist in the registry).
        dataview: String,
    },

    /// Handler that runs a WASM CodeComponent.
    Codecomponent {
        /// Source language: "javascript", "typescript", "wasm".
        language: String,
        /// Module file path relative to the app directory.
        module: String,
        /// Function name to call within the module.
        entrypoint: String,
        /// Datasource/DataView resource names the handler may access.
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
    /// Query-string parameter mappings (`?page=1` -> DataView param).
    #[serde(default)]
    pub query: HashMap<String, String>,

    /// URL path parameter mappings (`:id` -> DataView param).
    #[serde(default)]
    pub path: HashMap<String, String>,

    /// Body parameter mapping for write operations (POST/PUT/PATCH).
    #[serde(default)]
    pub body: HashMap<String, String>,

    /// Header parameter mappings (X-Tenant-Id -> DataView param).
    #[serde(default)]
    pub header: HashMap<String, String>,
}

/// Pipeline event handlers for a view.
///
/// Collapsed 4-stage pipeline per `rivers-technology-path-spec.md`:
///   pre_process → handlers → post_process + on_error
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct ViewEventHandlers {
    /// Pre-processing stage — runs before the primary handler.
    #[serde(default)]
    pub pre_process: Vec<HandlerStageConfig>,

    /// Ordered handler chain — replaces the former transform/on_response stages.
    #[serde(default)]
    pub handlers: Vec<HandlerStageConfig>,

    /// Post-processing stage — runs after successful handler execution.
    #[serde(default)]
    pub post_process: Vec<HandlerStageConfig>,

    /// Error recovery stage — runs when any prior stage fails.
    #[serde(default)]
    pub on_error: Vec<HandlerStageConfig>,
}

/// A single pipeline stage handler reference.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HandlerStageConfig {
    /// CodeComponent module path.
    pub module: String,
    /// Function name within the module.
    pub entrypoint: String,
    /// Optional key name for storing stage output in the pipeline context.
    pub key: Option<String>,
    /// Failure action: "abort" (default), "continue", or "redirect".
    pub on_failure: Option<String>,
}

/// `[api.views.*.on_stream]` — WebSocket stream handler.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OnStreamConfig {
    /// CodeComponent module path.
    pub module: String,
    /// Function name within the module.
    pub entrypoint: String,
    /// Handler mode: "Stream", "Normal", or "Auto".
    pub handler_mode: Option<String>,
}

/// WebSocket lifecycle hooks per technology-path-spec §14.2.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct WebSocketHooks {
    /// Called when a new WebSocket connection is established.
    pub on_connect: Option<HandlerStageConfig>,
    /// Called for each incoming WebSocket message.
    pub on_message: Option<HandlerStageConfig>,
    /// Called when a WebSocket connection closes.
    pub on_disconnect: Option<HandlerStageConfig>,
}

/// `[api.views.*.on_event]` — MessageConsumer event handler.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OnEventConfig {
    /// Topic or queue name to consume from.
    pub topic: String,
    /// CodeComponent handler function name.
    pub handler: String,
    /// Handler mode: "Stream", "Normal", or "Auto".
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

fn default_true() -> bool { true }

/// `on_change` handler — invoked when polled data changes.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct OnChangeConfig {
    /// CodeComponent module path.
    pub module: String,
    /// Function name within the module.
    pub entrypoint: String,
}

/// `change_detect` handler — custom diff logic via CodeComponent.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ChangeDetectConfig {
    /// CodeComponent module path.
    pub module: String,
    /// Function name within the module.
    pub entrypoint: String,
}

// ── MCP Config Types ─────────────────────────────────────

/// MCP tool declaration — maps a DataView or a codecomponent view to an MCP tool.
///
/// Exactly one of `dataview` or `view` must be set:
/// - `dataview`: dispatches through the DataView executor (existing behavior).
/// - `view`: dispatches through the ProcessPool like a REST codecomponent handler.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpToolConfig {
    /// Target DataView name. Empty string when `view` is set instead.
    #[serde(default)]
    pub dataview: String,
    /// Target codecomponent view name (alternative to `dataview`).
    #[serde(default)]
    pub view: Option<String>,
    /// Path to a JSON Schema file (relative to app bundle root) describing the tool's input.
    /// Loaded at tools/list time and served as the MCP `inputSchema` for codecomponent-backed tools.
    #[serde(default)]
    pub input_schema: Option<String>,
    /// Human-readable description for the AI model.
    #[serde(default)]
    pub description: String,
    /// HTTP method (GET/POST/PUT/DELETE) when DataView supports multiple.
    #[serde(default)]
    pub method: Option<String>,
    /// Tool behavior hints for the AI model.
    #[serde(default)]
    pub hints: McpToolHints,
}

/// MCP tool behavior hints.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct McpToolHints {
    /// Tool does not modify state.
    #[serde(default)]
    pub read_only: bool,
    /// Tool may perform destructive operations.
    #[serde(default = "default_true")]
    pub destructive: bool,
    /// Safe to retry without side effects.
    #[serde(default)]
    pub idempotent: bool,
    /// Tool interacts with external systems.
    #[serde(default = "default_true")]
    pub open_world: bool,
}

impl Default for McpToolHints {
    fn default() -> Self {
        Self {
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        }
    }
}

/// MCP resource declaration — read-only DataView exposure.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpResourceConfig {
    /// Target DataView name (GET method only).
    pub dataview: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// MIME type for the resource. Default: "application/json".
    #[serde(default = "default_mime")]
    pub mime_type: String,
    /// Optional RFC 6570 URI template (e.g. `cb://{project_id}/decisions{?since,limit}`).
    /// When set, served in `resources/templates/list` instead of the default
    /// `rivers://<app_id>/<name>` URI. Variables in the template are extracted
    /// from the incoming URI at `resources/read` time and passed as DataView params.
    #[serde(default)]
    pub uri_template: Option<String>,

    /// When true, this resource accepts `resources/subscribe` requests and
    /// emits `notifications/resources/updated` when underlying data changes.
    #[serde(default)]
    pub subscribable: bool,

    /// Polling interval in seconds for change detection. Default: 5.
    /// Ignored when `subscribable = false`.
    #[serde(default = "default_poll_interval_seconds")]
    pub poll_interval_seconds: u64,
}

fn default_mime() -> String { "application/json".into() }

fn default_poll_interval_seconds() -> u64 { 5 }

/// MCP prompt declaration — markdown template with argument substitution.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpPromptConfig {
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Path to markdown template file (relative to app bundle root).
    #[serde(default)]
    pub template: String,
    /// Prompt arguments for template substitution.
    #[serde(default)]
    pub arguments: Vec<McpPromptArgument>,
}

/// A single prompt argument.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpPromptArgument {
    /// Argument name (matches {placeholder} in template).
    pub name: String,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Whether this argument is required.
    #[serde(default)]
    pub required: bool,
    /// Default value when not provided.
    #[serde(default)]
    pub default: Option<String>,
}

/// MCP session configuration.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct McpSessionConfig {
    /// Session TTL in seconds. Default: 3600 (1 hour).
    #[serde(default = "default_mcp_ttl")]
    pub ttl_seconds: u64,
}

impl Default for McpSessionConfig {
    fn default() -> Self {
        Self { ttl_seconds: 3600 }
    }
}

fn default_mcp_ttl() -> u64 { 3600 }

// ── MCP Federation Config Types ──────────────────────────────────

/// A single federated MCP upstream declaration.
///
/// Declared in `app.toml` under `[api.views.*.federation.*]` or as an array.
/// Each entry causes the local MCP server to fetch and namespace the upstream's
/// tools/resources, routing calls back to the upstream transparently.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
pub struct McpFederationConfig {
    /// Short alias used to namespace federated tools/resources (e.g. "cb_service").
    /// Must match `[a-z0-9_]+`.
    pub alias: String,
    /// Base URL of the upstream MCP server (e.g. "http://localhost:9090/mcp/app-name").
    pub url: String,
    /// Bearer token for authenticating to the upstream. Optional.
    #[serde(default)]
    pub bearer_token: Option<String>,
    /// If non-empty, only these tool names are imported. Empty = import all.
    #[serde(default)]
    pub tools_filter: Vec<String>,
    /// If non-empty, only resources with URIs matching these prefixes are imported. Empty = all.
    #[serde(default)]
    pub resources_filter: Vec<String>,
    /// Request timeout in milliseconds. Default: 5000.
    #[serde(default = "default_federation_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_federation_timeout_ms() -> u64 { 5000 }
