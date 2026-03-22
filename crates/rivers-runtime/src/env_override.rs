//! Environment override application.
//!
//! Per `rivers-httpd-spec.md` §19.6 — environment-specific config values
//! are merged over the base config at startup.

use rivers_core_config::ServerConfig;

/// Apply environment-specific overrides to a `ServerConfig`.
///
/// Looks up `config.environment_overrides[env]` and merges any
/// non-None fields over the base config. The override is consumed.
pub fn apply_environment_overrides(config: &mut ServerConfig, env: &str) {
    let overrides = match config.environment_overrides.remove(env) {
        Some(o) => o,
        None => return,
    };

    // Base overrides
    if let Some(base) = overrides.base {
        if let Some(host) = base.host {
            config.base.host = host;
        }
        if let Some(port) = base.port {
            config.base.port = port;
        }
        if let Some(workers) = base.workers {
            config.base.workers = Some(workers);
        }
        if let Some(timeout) = base.request_timeout_seconds {
            config.base.request_timeout_seconds = timeout;
        }
        if let Some(level) = base.log_level {
            config.base.log_level = level;
        }
        if let Some(bp) = base.backpressure {
            if let Some(enabled) = bp.enabled {
                config.base.backpressure.enabled = enabled;
            }
            if let Some(depth) = bp.queue_depth {
                config.base.backpressure.queue_depth = depth;
            }
            if let Some(timeout) = bp.queue_timeout_ms {
                config.base.backpressure.queue_timeout_ms = timeout;
            }
        }
    }

    // Security overrides
    if let Some(sec) = overrides.security {
        if let Some(cors) = sec.cors_enabled {
            config.security.cors_enabled = cors;
        }
        if let Some(origins) = sec.cors_allowed_origins {
            config.security.cors_allowed_origins = origins;
        }
        if let Some(rate) = sec.rate_limit_per_minute {
            config.security.rate_limit_per_minute = rate;
        }
        if let Some(burst) = sec.rate_limit_burst_size {
            config.security.rate_limit_burst_size = burst;
        }
    }

    // StorageEngine overrides
    if let Some(se) = overrides.storage_engine {
        if let Some(backend) = se.backend {
            config.storage_engine.backend = backend;
        }
        if let Some(url) = se.url {
            config.storage_engine.url = Some(url);
        }
        if let Some(creds) = se.credentials_source {
            config.storage_engine.credentials_source = Some(creds);
        }
        if let Some(prefix) = se.key_prefix {
            config.storage_engine.key_prefix = Some(prefix);
        }
        if let Some(size) = se.pool_size {
            config.storage_engine.pool_size = Some(size);
        }
    }
}
