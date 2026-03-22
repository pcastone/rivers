//! Token bucket rate limiter.
//!
//! Per `rivers-httpd-spec.md` §10.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Mutex;

/// Maximum number of rate limit buckets before eviction.
const RATE_LIMIT_MAX_BUCKETS: usize = 10_000;

/// Stale bucket threshold in milliseconds (5 minutes).
const STALE_THRESHOLD_MS: u128 = 5 * 60 * 1000;

/// Rate limit strategy — how to identify clients.
///
/// Per spec §10.2.
#[derive(Debug, Clone, Default)]
pub enum RateLimitStrategy {
    /// Use remote IP address (default).
    #[default]
    Ip,
    /// Use value from a custom header. Falls back to IP if header absent.
    CustomHeader(String),
}

/// Rate limiter configuration.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Requests allowed per minute.
    pub requests_per_minute: u32,
    /// Burst size (max tokens / bucket capacity).
    pub burst_size: u32,
    /// Strategy for identifying clients.
    pub strategy: RateLimitStrategy,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_minute: 120,
            burst_size: 60,
            strategy: RateLimitStrategy::Ip,
        }
    }
}

/// A single client's token bucket state.
struct Bucket {
    tokens: f64,
    last_refill: Instant,
}

/// Result of checking a rate limit.
pub enum RateLimitResult {
    /// Request is allowed.
    Allowed,
    /// Request is rate limited. Contains Retry-After in seconds.
    Limited { retry_after_secs: u64 },
}

/// Token bucket rate limiter.
///
/// Per spec §10.1.
pub struct RateLimiter {
    buckets: Mutex<HashMap<String, Bucket>>,
    capacity: f64,
    refill_rate_per_ms: f64,
}

impl RateLimiter {
    /// Create a new rate limiter with the given config.
    pub fn new(config: &RateLimitConfig) -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            capacity: config.burst_size as f64,
            refill_rate_per_ms: config.requests_per_minute as f64 / 60_000.0,
        }
    }

    /// Check if a request is allowed for the given key.
    ///
    /// Per spec §10.1 token bucket algorithm.
    pub async fn check(&self, key: &str) -> RateLimitResult {
        let mut buckets = self.buckets.lock().await;
        let now = Instant::now();

        // Evict if over limit
        if buckets.len() >= RATE_LIMIT_MAX_BUCKETS {
            evict_stale_buckets(&mut buckets, now);
            // If still over, evict oldest 50%
            if buckets.len() >= RATE_LIMIT_MAX_BUCKETS {
                evict_oldest_half(&mut buckets);
            }
        }

        let bucket = buckets.entry(key.to_string()).or_insert_with(|| Bucket {
            tokens: self.capacity,
            last_refill: now,
        });

        // Refill tokens based on elapsed time
        let elapsed_ms = now.duration_since(bucket.last_refill).as_millis() as f64;
        bucket.tokens += elapsed_ms * self.refill_rate_per_ms;
        bucket.tokens = bucket.tokens.min(self.capacity);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            RateLimitResult::Allowed
        } else {
            // Calculate retry-after in seconds
            let deficit = 1.0 - bucket.tokens;
            let retry_ms = (deficit / self.refill_rate_per_ms).ceil();
            let retry_secs = ((retry_ms / 1000.0).ceil() as u64).max(1);
            RateLimitResult::Limited {
                retry_after_secs: retry_secs,
            }
        }
    }

    /// Number of active buckets (for testing/monitoring).
    pub async fn bucket_count(&self) -> usize {
        self.buckets.lock().await.len()
    }
}

/// Evict buckets last seen more than 5 minutes ago.
///
/// Per spec §10.3 step 1.
fn evict_stale_buckets(buckets: &mut HashMap<String, Bucket>, now: Instant) {
    buckets.retain(|_, b| now.duration_since(b.last_refill).as_millis() < STALE_THRESHOLD_MS);
}

/// Evict oldest 50% of buckets by last_refill time.
///
/// Per spec §10.3 step 2.
fn evict_oldest_half(buckets: &mut HashMap<String, Bucket>) {
    let count = buckets.len();
    let remove_count = count / 2;
    if remove_count == 0 {
        return;
    }

    // Collect keys sorted by last_refill (oldest first)
    let mut entries: Vec<(String, Instant)> = buckets
        .iter()
        .map(|(k, b)| (k.clone(), b.last_refill))
        .collect();
    entries.sort_by_key(|(_, t)| *t);

    // Remove oldest half
    for (key, _) in entries.into_iter().take(remove_count) {
        buckets.remove(&key);
    }
}

/// Per-view rate limiter that wraps a global limiter with view-specific config.
///
/// Per spec §10.4.
pub struct PerViewRateLimiter {
    /// Global rate limiter.
    pub global: Arc<RateLimiter>,
    /// Per-view rate limiters, keyed by view ID.
    view_limiters: Mutex<HashMap<String, Arc<RateLimiter>>>,
}

impl PerViewRateLimiter {
    pub fn new(global: Arc<RateLimiter>) -> Self {
        Self {
            global,
            view_limiters: Mutex::new(HashMap::new()),
        }
    }

    /// Check rate limit for a request. If the view has a per-view override,
    /// use that; otherwise use the global limiter.
    pub async fn check(
        &self,
        key: &str,
        view_id: Option<&str>,
        view_config: Option<&ViewRateLimitConfig>,
    ) -> RateLimitResult {
        if let (Some(vid), Some(cfg)) = (view_id, view_config) {
            if cfg.rate_limit_per_minute.is_some() || cfg.rate_limit_burst_size.is_some() {
                let limiter = self.get_or_create_view_limiter(vid, cfg).await;
                // Per-view key includes view ID for isolation
                let view_key = format!("{}:{}", vid, key);
                return limiter.check(&view_key).await;
            }
        }
        self.global.check(key).await
    }

    async fn get_or_create_view_limiter(
        &self,
        view_id: &str,
        cfg: &ViewRateLimitConfig,
    ) -> Arc<RateLimiter> {
        let mut limiters = self.view_limiters.lock().await;
        if let Some(limiter) = limiters.get(view_id) {
            return limiter.clone();
        }

        let config = RateLimitConfig {
            requests_per_minute: cfg.rate_limit_per_minute.unwrap_or(120),
            burst_size: cfg.rate_limit_burst_size.unwrap_or(60),
            strategy: RateLimitStrategy::Ip,
        };
        let limiter = Arc::new(RateLimiter::new(&config));
        limiters.insert(view_id.to_string(), limiter.clone());
        limiter
    }
}

/// Per-view rate limit override config.
///
/// Per spec §10.4.
#[derive(Debug, Clone, Default)]
pub struct ViewRateLimitConfig {
    pub rate_limit_per_minute: Option<u32>,
    pub rate_limit_burst_size: Option<u32>,
}
