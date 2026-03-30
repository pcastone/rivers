//! Router construction — main and admin server routers.

use std::sync::Arc;

use axum::middleware as axum_middleware;
use axum::routing::get;
use axum::Router;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

use crate::backpressure::BackpressureState;
use crate::cors::CorsConfig;
use crate::middleware;

use super::context::AppContext;
use super::handlers::{health_handler, health_verbose_handler, gossip_receive_handler};
use super::view_dispatch::combined_fallback_handler;
use super::admin_auth::admin_auth_middleware;

use crate::admin_handlers::{
    admin_status_handler, admin_drivers_handler, admin_datasources_handler,
    admin_deploy_handler, admin_deploy_test_handler,
    admin_deploy_approve_handler, admin_deploy_reject_handler,
    admin_deploy_promote_handler, admin_deployments_handler,
    admin_log_levels_handler, admin_log_set_handler, admin_log_reset_handler,
    admin_shutdown_handler,
};

/// Build the main server router.
///
/// Per spec §3: route registration order —
/// health → gossip → graphql → views → static.
pub fn build_main_router(ctx: AppContext) -> Router {
    let shutdown = ctx.shutdown.clone();
    let timeout_secs = ctx.config.base.request_timeout_seconds;
    let cors_config = Arc::new(CorsConfig {
        enabled: ctx.config.security.cors_enabled,
        allowed_origins: ctx.config.security.cors_allowed_origins.clone(),
        allowed_methods: ctx.config.security.cors_allowed_methods.clone(),
        allowed_headers: ctx.config.security.cors_allowed_headers.clone(),
        allow_credentials: ctx.config.security.cors_allow_credentials,
    });

    // Backpressure state — per spec §11
    let bp_config = &ctx.config.base.backpressure;
    let backpressure = BackpressureState::new(
        bp_config.queue_depth,
        bp_config.queue_timeout_ms,
        bp_config.enabled,
    );

    // Route registration order per spec §3
    let mut app = Router::new()
        // 1. Health endpoints
        .route("/health", get(health_handler))
        .route("/health/verbose", get(health_verbose_handler))
        // 2. Gossip endpoint (always registered per spec §3)
        .route("/gossip/receive", axum::routing::post(gossip_receive_handler));

    // 3. GraphQL routes — mount if schema is built
    if ctx.config.graphql.enabled {
        let gql_schema = ctx.graphql_schema.clone();
        // Try to get the schema synchronously — it was built at bundle load
        let maybe_schema = gql_schema.try_read().ok().and_then(|g| g.clone());
        if let Some(schema) = maybe_schema {
            let gql_config = crate::graphql::GraphqlConfig::from(&ctx.config.graphql);
            let gql_path = gql_config.path.clone();
            let introspection = gql_config.introspection;

            let schema_for_post = schema.clone();
            let post_handler = axum::routing::post(move |req: async_graphql_axum::GraphQLRequest| {
                let schema = schema_for_post.clone();
                async move {
                    let resp = schema.execute(req.into_inner()).await;
                    async_graphql_axum::GraphQLResponse::from(resp)
                }
            });
            app = app.route(&gql_path, post_handler);

            if introspection {
                let playground_path = format!("{}/playground", gql_path.trim_end_matches('/'));
                let playground_handler = axum::routing::get(|| async {
                    axum::response::Html(
                        async_graphql::http::playground_source(
                            async_graphql::http::GraphQLPlaygroundConfig::new("/graphql"),
                        ),
                    )
                });
                app = app.route(&playground_path, playground_handler);
            }

            tracing::info!(path = %gql_path, "GraphQL endpoint mounted");
        }
    }

    // 4. View routes + 5. Static file fallback
    // Combined fallback: tries view dispatch first, then static files.
    // View routes are matched dynamically after bundle deployment.
    let app = app
        .fallback(combined_fallback_handler)
        .with_state(ctx);

    // Middleware stack per spec §4 (layers apply in reverse — last = outermost)
    //
    // Innermost to outermost:
    // 10. request_observer
    // 9. timeout
    // 8. backpressure
    // 7. shutdown_guard
    // 6. rate_limit (pass-through — wired per-view at dispatch time)
    // 5. session (per-view — checked at view dispatch, not globally)
    // 4. security_headers
    // 3. trace_id
    // 2. body_limit (16 MiB)
    // 1. cors (covers all responses including errors)
    // 0. compression (outermost)
    app.layer(axum_middleware::from_fn(
            middleware::request_observer_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            timeout_secs,
            middleware::timeout_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            backpressure,
            crate::backpressure::backpressure_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            shutdown,
            middleware::shutdown_guard_middleware,
        ))
        .layer(axum_middleware::from_fn(
            middleware::security_headers_middleware,
        ))
        .layer(axum_middleware::from_fn(middleware::trace_id_middleware))
        .layer(RequestBodyLimitLayer::new(16 * 1024 * 1024)) // 16 MiB
        .layer(axum_middleware::from_fn_with_state(
            cors_config,
            middleware::cors_middleware,
        ))
        .layer(CompressionLayer::new())
}

/// Build the admin server router.
///
/// Per spec §1, §15: admin server has subset middleware + auth.
pub fn build_admin_router(ctx: AppContext) -> Router {
    let timeout_secs = ctx.config.base.request_timeout_seconds;
    let auth_ctx = ctx.clone();

    let app = Router::new()
        // Status/info endpoints
        .route("/admin/status", get(admin_status_handler))
        .route("/admin/drivers", get(admin_drivers_handler))
        .route("/admin/datasources", get(admin_datasources_handler))
        // Deployment lifecycle endpoints per spec §15.6
        .route("/admin/deploy", axum::routing::post(admin_deploy_handler))
        .route("/admin/deploy/test", axum::routing::post(admin_deploy_test_handler))
        .route("/admin/deploy/approve", axum::routing::post(admin_deploy_approve_handler))
        .route("/admin/deploy/reject", axum::routing::post(admin_deploy_reject_handler))
        .route("/admin/deploy/promote", axum::routing::post(admin_deploy_promote_handler))
        .route("/admin/deployments", get(admin_deployments_handler))
        // Log management endpoints per spec §15.8
        .route("/admin/log/levels", get(admin_log_levels_handler))
        .route("/admin/log/set", axum::routing::post(admin_log_set_handler))
        .route("/admin/log/reset", axum::routing::post(admin_log_reset_handler))
        // Shutdown endpoint
        .route("/admin/shutdown", axum::routing::post(admin_shutdown_handler))
        .with_state(ctx);

    // Admin middleware: admin_auth → timeout → security_headers → trace_id → body_limit
    app.layer(axum_middleware::from_fn_with_state(
            timeout_secs,
            middleware::timeout_middleware,
        ))
        .layer(axum_middleware::from_fn_with_state(
            auth_ctx,
            admin_auth_middleware,
        ))
        .layer(axum_middleware::from_fn(
            middleware::security_headers_middleware,
        ))
        .layer(axum_middleware::from_fn(middleware::trace_id_middleware))
        .layer(RequestBodyLimitLayer::new(16 * 1024 * 1024))
}
