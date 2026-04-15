use std::collections::HashMap;
use std::sync::Arc;

use rivers_runtime::view::ApiViewConfig;
use riversd::process_pool::ProcessPoolManager;

pub fn default_pool() -> Arc<ProcessPoolManager> {
    Arc::new(ProcessPoolManager::from_config(&HashMap::new()))
}

pub fn default_event_bus() -> Arc<rivers_runtime::rivers_core::EventBus> {
    Arc::new(rivers_runtime::rivers_core::EventBus::new())
}

/// Helper for constructing test ApiViewConfig with defaults.
pub fn default_view_config() -> ApiViewConfig {
    use rivers_runtime::view::HandlerConfig;
    ApiViewConfig {
        view_type: "Rest".into(),
        path: None,
        method: None,
        handler: HandlerConfig::None {},
        parameter_mapping: None,
        dataviews: vec![],
        primary: None,
        streaming: None,
        streaming_format: None,
        stream_timeout_ms: None,
        guard: false,
        auth: None,
        guard_config: None,
        allow_outbound_http: false,
        rate_limit_per_minute: None,
        rate_limit_burst_size: None,
        websocket_mode: None,
        max_connections: None,
        sse_tick_interval_ms: None,
        sse_trigger_events: vec![],
        sse_event_buffer_size: None,
        session_revalidation_interval_s: None,
        polling: None,
        event_handlers: None,
        on_stream: None,
        ws_hooks: None,
        on_event: None,
        tools: HashMap::new(),
        resources: HashMap::new(),
        prompts: HashMap::new(),
        instructions: None,
        session: None,
    }
}
