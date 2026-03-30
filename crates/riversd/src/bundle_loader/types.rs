//! Type definitions and EventHandler implementations for bundle loading.

use std::sync::Arc;

use async_trait::async_trait;

// ── SSE Trigger Handler ─────────────────────────────────────────────

/// EventHandler that pushes an SSE event when a trigger event fires on the EventBus.
///
/// Registered per trigger-event per SSE view during bundle loading.
#[allow(dead_code)]
pub(crate) struct SseTriggerHandler {
    pub(crate) channel: Arc<crate::sse::SseChannel>,
    pub(crate) view_id: String,
}

#[async_trait]
impl rivers_runtime::rivers_core::eventbus::EventHandler for SseTriggerHandler {
    async fn handle(&self, event: &rivers_runtime::rivers_core::event::Event) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let sse_event = crate::sse::SseEvent::typed(
            event.event_type.clone(),
            serde_json::to_string(&event.payload).unwrap_or_else(|_| "{}".to_string()),
        );
        // Ignore NoActiveClients — no subscribers connected yet is fine
        let _ = self.channel.push(sse_event);
        Ok(())
    }

    fn name(&self) -> &str {
        "SseTriggerHandler"
    }
}

// ── Hot Reload Summary ──────────────────────────────────────────────

/// Summary of a hot reload rebuild.
#[derive(Debug)]
pub struct ReloadSummary {
    pub apps: usize,
    pub views: usize,
    pub dataviews: usize,
}

// ── Datasource Event Handler ──────────────────────────────────────

/// EventBus handler that dispatches datasource failure events to a CodeComponent.
pub(crate) struct DatasourceEventBusHandler {
    pub(crate) datasource: String,
    pub(crate) module: String,
    pub(crate) entrypoint: String,
    pub(crate) pool: Arc<crate::process_pool::ProcessPoolManager>,
}

#[async_trait::async_trait]
impl rivers_runtime::rivers_core::eventbus::EventHandler for DatasourceEventBusHandler {
    async fn handle(
        &self,
        event: &rivers_runtime::rivers_core::event::Event,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let entrypoint = crate::process_pool::Entrypoint {
            module: self.module.clone(),
            function: self.entrypoint.clone(),
            language: "javascript".into(),
        };

        let args = serde_json::json!({
            "datasource": self.datasource,
            "event_type": event.event_type,
            "event": event.payload,
            "trace_id": event.trace_id,
            "timestamp": event.timestamp.to_rfc3339(),
        });

        let builder = crate::process_pool::TaskContextBuilder::new()
            .entrypoint(entrypoint)
            .args(args)
            .trace_id(event.trace_id.clone().unwrap_or_default());
        let builder = crate::task_enrichment::enrich(builder, "");
        let task_ctx = builder
            .build()
            .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())))?;

        self.pool.dispatch("default", task_ctx).await.map_err(|e| {
            Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        })?;

        Ok(())
    }

    fn name(&self) -> &str {
        &self.datasource
    }
}
