//! Streaming and event wiring for loaded bundles.
//!
//! Contains the second half of bundle wiring: broker bridges,
//! message consumers, SSE/WS managers, and datasource event handlers.

use std::sync::Arc;

use rivers_runtime::rivers_core::DriverFactory;

use crate::server::{AppContext, ServerError};
use super::types::{SseTriggerHandler, DatasourceEventBusHandler};

/// Wire broker bridges, message consumers, SSE/WS channels, and
/// datasource event handlers for a loaded bundle.
///
/// This is the second phase of `load_and_wire_bundle`, called after
/// DataViews, drivers, GraphQL, and guard views have been set up.
pub(crate) async fn wire_streaming_and_events(
    ctx: &mut AppContext,
    bundle: &rivers_runtime::LoadedBundle,
    factory: &Arc<DriverFactory>,
    ds_params: &Arc<std::collections::HashMap<String, rivers_runtime::rivers_driver_sdk::ConnectionParams>>,
    shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<(), ServerError> {
    // ── AL2: Wire broker consumer bridges + MessageConsumer handlers ──
    // Scan apps for broker datasources and MessageConsumer views.
    let mut broker_bridge_count = 0usize;
    let mut consumer_count = 0usize;

    for app in &bundle.apps {
        let entry_point = app.manifest.entry_point.as_deref()
            .unwrap_or(&app.manifest.app_name);

        // AL2.1: Find broker datasources in this app's resources
        for ds in &app.resources.datasources {
            if let Some(broker_driver) = factory.get_broker_driver(&ds.driver) {
                // AL2.2: Create broker consumer and spawn bridge
                let namespaced_key = format!("{}:{}", entry_point, ds.name);
                if let Some(params) = ds_params.get(&namespaced_key) {
                    // Collect subscriptions from MessageConsumer views targeting this datasource
                    let mut subscriptions = Vec::new();
                    for (view_id, view_cfg) in &app.config.api.views {
                        if view_cfg.view_type == "MessageConsumer" {
                            // Use the view_id as the topic name for the subscription
                            subscriptions.push(
                                rivers_runtime::rivers_driver_sdk::broker::BrokerSubscription {
                                    topic: view_id.clone(),
                                    event_name: Some(view_id.clone()),
                                },
                            );
                        }
                    }

                    if subscriptions.is_empty() {
                        continue;
                    }

                    // Read consumer config from the full DatasourceConfig (app.toml)
                    let full_ds_config = app.config.data.datasources.get(&ds.name);
                    let consumer_cfg = full_ds_config.and_then(|d| d.consumer.as_ref());

                    let group_prefix = consumer_cfg
                        .and_then(|c| c.group_prefix.as_deref())
                        .unwrap_or("rivers")
                        .to_string();
                    let reconnect_ms = consumer_cfg
                        .map(|c| c.reconnect_ms)
                        .unwrap_or(5000);

                    // Build failure policy from config (default: Drop)
                    let failure_policy = consumer_cfg
                        .and_then(|c| c.subscriptions.first())
                        .and_then(|s| s.on_failure.as_ref())
                        .map(|fp| {
                            let mode = match fp.mode.as_str() {
                                "dead_letter" => rivers_runtime::rivers_driver_sdk::broker::FailureMode::DeadLetter,
                                "requeue" => rivers_runtime::rivers_driver_sdk::broker::FailureMode::Requeue,
                                "redirect" => rivers_runtime::rivers_driver_sdk::broker::FailureMode::Redirect,
                                _ => rivers_runtime::rivers_driver_sdk::broker::FailureMode::Drop,
                            };
                            rivers_runtime::rivers_driver_sdk::broker::FailurePolicy {
                                mode,
                                destination: fp.destination.clone(),
                                handlers: Vec::new(),
                            }
                        })
                        .unwrap_or(rivers_runtime::rivers_driver_sdk::broker::FailurePolicy {
                            mode: rivers_runtime::rivers_driver_sdk::broker::FailureMode::Drop,
                            destination: None,
                            handlers: Vec::new(),
                        });

                    // Warn if manual ack mode is configured (not yet supported)
                    if let Some(cfg) = consumer_cfg {
                        for sub in &cfg.subscriptions {
                            if sub.ack_mode == "manual" {
                                tracing::warn!(
                                    datasource = %ds.name,
                                    topic = %sub.topic,
                                    "ack_mode='manual' is not yet supported — using 'auto'"
                                );
                            }
                        }
                    }

                    let broker_config = rivers_runtime::rivers_driver_sdk::broker::BrokerConsumerConfig {
                        group_prefix,
                        app_id: app.manifest.app_id.clone(),
                        datasource_id: ds.name.clone(),
                        node_id: "node-0".to_string(),
                        reconnect_ms,
                        subscriptions,
                    };

                    // Nonblocking: spawn supervisor that owns connect+retry+run.
                    // wire_streaming_and_events returns immediately so HTTP
                    // listener bind isn't gated on broker reachability.
                    // See code review P0-4 / todo/tasks.md A1.1.
                    let backoff = crate::broker_supervisor::SupervisorBackoff::from_reconnect_ms(
                        reconnect_ms,
                    );
                    crate::broker_supervisor::spawn_broker_supervisor(
                        broker_driver.clone(),
                        params.clone(),
                        broker_config,
                        failure_policy,
                        ctx.event_bus.clone(),
                        ds.name.clone(),
                        ds.driver.clone(),
                        backoff,
                        ctx.broker_bridge_registry.clone(),
                        shutdown_rx.clone(),
                    );
                    broker_bridge_count += 1;
                    tracing::info!(
                        datasource = %ds.name,
                        driver = %ds.driver,
                        "broker supervisor spawned (nonblocking)"
                    );
                }
            }
        }

        // AL2.3: Build MessageConsumerRegistry and subscribe handlers
        let mc_registry = crate::message_consumer::MessageConsumerRegistry::from_views(
            &app.config.api.views,
            entry_point,
        );
        if !mc_registry.is_empty() {
            consumer_count += mc_registry.len();
            crate::message_consumer::subscribe_message_consumers(
                &mc_registry,
                &ctx.event_bus,
                ctx.pool.clone(),
            )
            .await;
        }
    }

    if broker_bridge_count > 0 || consumer_count > 0 {
        tracing::info!(
            bridges = broker_bridge_count,
            consumers = consumer_count,
            "broker pipeline wired"
        );
    }

    // ── AL3: Wire datasource event handlers ──
    // Subscribe CodeComponent handlers to EventBus events for datasource failures.
    {
        use rivers_runtime::rivers_core::eventbus::{events, HandlerPriority};
        let mut ds_handler_count = 0usize;

        for app in &bundle.apps {
            let app_entry_point = app.manifest.entry_point.as_deref()
                .unwrap_or(&app.manifest.app_name)
                .to_string();
            for ds in app.config.data.datasources.values() {
                if let Some(ref handlers) = ds.event_handlers {
                    // on_connection_failed → DatasourceCircuitOpened + DatasourceHealthCheckFailed
                    for handler_ref in &handlers.on_connection_failed {
                        let handler = Arc::new(DatasourceEventBusHandler {
                            datasource: ds.name.clone(),
                            module: handler_ref.module.clone(),
                            entrypoint: handler_ref.entrypoint.clone(),
                            pool: ctx.pool.clone(),
                            app_id: app_entry_point.clone(),
                        });
                        ctx.event_bus
                            .subscribe(events::DATASOURCE_CIRCUIT_OPENED.to_string(), handler.clone(), HandlerPriority::Handle)
                            .await;
                        ctx.event_bus
                            .subscribe(events::DATASOURCE_HEALTH_CHECK_FAILED.to_string(), handler, HandlerPriority::Handle)
                            .await;
                        ds_handler_count += 1;
                        tracing::info!(
                            datasource = %ds.name,
                            module = %handler_ref.module,
                            entrypoint = %handler_ref.entrypoint,
                            "on_connection_failed handler subscribed"
                        );
                    }

                    // on_pool_exhausted → ConnectionPoolExhausted
                    for handler_ref in &handlers.on_pool_exhausted {
                        let handler = Arc::new(DatasourceEventBusHandler {
                            datasource: ds.name.clone(),
                            module: handler_ref.module.clone(),
                            entrypoint: handler_ref.entrypoint.clone(),
                            pool: ctx.pool.clone(),
                            app_id: app_entry_point.clone(),
                        });
                        ctx.event_bus
                            .subscribe(events::CONNECTION_POOL_EXHAUSTED.to_string(), handler, HandlerPriority::Handle)
                            .await;
                        ds_handler_count += 1;
                        tracing::info!(
                            datasource = %ds.name,
                            module = %handler_ref.module,
                            entrypoint = %handler_ref.entrypoint,
                            "on_pool_exhausted handler subscribed"
                        );
                    }
                }
            }
        }
        if ds_handler_count > 0 {
            tracing::info!(handlers = ds_handler_count, "datasource event handlers wired");
        }
    }

    // ── Wire SSE and WebSocket view managers ──
    let mut sse_count = 0usize;
    let mut ws_count = 0usize;

    for app in &bundle.apps {
        let entry_point = app.manifest.entry_point.as_deref()
            .unwrap_or(&app.manifest.app_name);

        for (view_id, view_cfg) in &app.config.api.views {
            let qualified_id = format!("{}:{}", entry_point, view_id);

            match view_cfg.view_type.as_str() {
                "ServerSentEvents" => {
                    let tick_ms = view_cfg.sse_tick_interval_ms.unwrap_or(0);
                    let triggers = view_cfg.sse_trigger_events.clone();
                    let max_conns = view_cfg.max_connections;

                    let buffer_size = view_cfg.sse_event_buffer_size.unwrap_or(100);
                    let channel = ctx.sse_manager.register_with_buffer(
                        qualified_id.clone(),
                        max_conns,
                        tick_ms,
                        triggers.clone(),
                        buffer_size,
                    ).await;

                    // Subscribe trigger events to EventBus → push to SSE channel
                    for event_name in &triggers {
                        let ch = channel.clone();
                        let handler = Arc::new(SseTriggerHandler {
                            channel: ch,
                            view_id: qualified_id.clone(),
                        });
                        ctx.event_bus.subscribe(
                            event_name.clone(),
                            handler,
                            rivers_runtime::rivers_core::eventbus::HandlerPriority::Handle,
                        ).await;
                    }

                    // Spawn channel-level push loop for SSE views
                    if view_cfg.polling.is_some() || tick_ms > 0 {
                        let ch = channel.clone();
                        let vid = qualified_id.clone();

                        if let Some(ref polling) = view_cfg.polling {
                            // Real DataView polling with StorageEngine persistence
                            let executor: Arc<dyn crate::polling::PollDataViewExecutor> = Arc::new(
                                crate::polling::DataViewPollExecutor::new(ctx.dataview_executor.clone())
                            );
                            let storage = ctx.storage_engine.clone();
                            let strategy = Some(crate::polling::DiffStrategy::from_str_opt(
                                Some(polling.diff_strategy.as_str())
                            ));
                            let poll_tick_ms = polling.tick_interval_ms;

                            tokio::spawn(async move {
                                crate::sse::drive_sse_push_loop(
                                    ch, poll_tick_ms, vid,
                                    Some(executor), storage, strategy,
                                ).await;
                            });
                        } else {
                            // Heartbeat mode — no DataView polling
                            tokio::spawn(async move {
                                crate::sse::drive_sse_push_loop(ch, tick_ms, vid, None, None, None).await;
                            });
                        }
                    }

                    sse_count += 1;
                    tracing::info!(
                        view_id = %qualified_id,
                        tick_ms = tick_ms,
                        triggers = triggers.len(),
                        "SSE channel registered"
                    );
                }
                "Websocket" => {
                    let mode = crate::websocket::WebSocketMode::from_str_opt(
                        view_cfg.websocket_mode.as_deref(),
                    );
                    let max_conns = view_cfg.max_connections;

                    match mode {
                        crate::websocket::WebSocketMode::Broadcast => {
                            ctx.ws_manager.register_broadcast(
                                qualified_id.clone(),
                                max_conns,
                            ).await;
                        }
                        crate::websocket::WebSocketMode::Direct => {
                            ctx.ws_manager.register_direct(
                                qualified_id.clone(),
                                max_conns,
                            ).await;
                        }
                    }

                    ws_count += 1;
                    tracing::info!(
                        view_id = %qualified_id,
                        mode = ?mode,
                        "WebSocket route registered"
                    );
                }
                _ => {}
            }
        }
    }

    if sse_count > 0 || ws_count > 0 {
        tracing::info!(
            sse_channels = sse_count,
            ws_routes = ws_count,
            "streaming views wired"
        );
    }

    Ok(())
}
