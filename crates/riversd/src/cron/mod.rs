//! Cron view scheduler — fire-and-forget scheduled tasks.
//!
//! Implements `view_type = "Cron"` per [`docs/arch/rivers-cron-view-spec.md`]
//! (CB-P1.14, Sprint 2026-05-09 Track 3). One `tokio` task per Cron view;
//! each loop computes the next scheduled instant, sleeps, attempts to win
//! a per-tick `set_if_absent` lock against the StorageEngine, and dispatches
//! the configured codecomponent handler via the ProcessPool.
//!
//! See spec §4 (tick lifecycle), §5 (multi-instance dedupe), §6 (overlap
//! policies), §9 (observability), §10 (failure semantics).

use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rivers_runtime::process_pool::TaskKind;
use rivers_runtime::rivers_core::storage::StorageEngine;
use rivers_runtime::ApiViewConfig;
use tokio::sync::Notify;

use crate::process_pool::{Entrypoint, ProcessPoolManager, TaskContextBuilder};

// ── Configuration parsing ──────────────────────────────────────────

/// Resolved next-tick strategy parsed from a Cron view's config.
#[derive(Debug, Clone)]
pub enum NextTick {
    /// 6-field cron expression (per `cron` crate's parser).
    Cron(cron::Schedule),
    /// Plain integer interval, computed from the loop's start time.
    Interval(std::time::Duration),
}

impl NextTick {
    /// Compute the next scheduled instant after `now` (UTC).
    pub fn next_after(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            NextTick::Cron(schedule) => schedule.after(&now).next(),
            NextTick::Interval(d) => Some(now + chrono::Duration::from_std(*d).ok()?),
        }
    }
}

/// Overlap policy per spec §6. Validated upstream — runtime treats unknown
/// strings as `Skip` (defensive default; the validator should have caught it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapPolicy {
    /// If the previous tick is still running, drop this tick.
    Skip,
    /// Push tick onto a bounded `mpsc`. Rejected ticks are metric'd + dropped.
    Queue,
    /// Spawn unconditionally. Caller's responsibility to be safe.
    Allow,
}

impl OverlapPolicy {
    fn from_str_or_default(s: Option<&str>) -> Self {
        match s {
            Some("queue") => OverlapPolicy::Queue,
            Some("allow") => OverlapPolicy::Allow,
            _ => OverlapPolicy::Skip,
        }
    }
}

/// Resolved per-view spec — what the scheduler needs to run a Cron loop.
/// Built from `ApiViewConfig`. Returns `Err` if the config can't yield a
/// valid spec (these all have validator coverage upstream — this is the
/// last-mile defensive check).
#[derive(Debug, Clone)]
pub struct CronViewSpec {
    /// App that owns this view (used for the StorageEngine dedupe key,
    /// per-app log routing, and capability namespace).
    pub app_id: String,
    /// View name as declared in `[api.views.<name>]`.
    pub view_name: String,
    /// Resolved schedule.
    pub schedule: NextTick,
    /// Resolved overlap policy (default: `Skip`).
    pub overlap: OverlapPolicy,
    /// Bounded queue capacity for `OverlapPolicy::Queue`. Default 16.
    pub max_concurrent: u32,
    /// Codecomponent module + entrypoint + language.
    pub entrypoint: Entrypoint,
}

impl CronViewSpec {
    /// Build a spec from a Cron view's config. Returns `None` if the view
    /// is not actually a Cron view.
    pub fn from_view_config(
        app_id: &str,
        view_name: &str,
        cfg: &ApiViewConfig,
    ) -> Result<Option<Self>, CronSpecError> {
        if cfg.view_type != "Cron" {
            return Ok(None);
        }

        let schedule = match (cfg.schedule.as_deref(), cfg.interval_seconds) {
            (Some(expr), None) => NextTick::Cron(
                cron::Schedule::from_str(expr).map_err(|e| {
                    CronSpecError::InvalidSchedule(format!("'{}': {}", expr, e))
                })?,
            ),
            (None, Some(secs)) if secs >= 1 => {
                NextTick::Interval(std::time::Duration::from_secs(secs))
            }
            (None, Some(_)) => return Err(CronSpecError::IntervalTooSmall),
            (Some(_), Some(_)) => return Err(CronSpecError::ScheduleAndIntervalBothSet),
            (None, None) => return Err(CronSpecError::NoSchedule),
        };

        let entrypoint = extract_entrypoint(cfg)
            .ok_or(CronSpecError::HandlerNotCodecomponent)?;

        Ok(Some(CronViewSpec {
            app_id: app_id.to_string(),
            view_name: view_name.to_string(),
            schedule,
            overlap: OverlapPolicy::from_str_or_default(cfg.overlap_policy.as_deref()),
            max_concurrent: cfg.max_concurrent.unwrap_or(16),
            entrypoint,
        }))
    }
}

fn extract_entrypoint(cfg: &ApiViewConfig) -> Option<Entrypoint> {
    use rivers_runtime::view::HandlerConfig;
    match &cfg.handler {
        HandlerConfig::Codecomponent {
            language,
            module,
            entrypoint,
            ..
        } => Some(Entrypoint {
            module: module.clone(),
            function: entrypoint.clone(),
            language: language.clone(),
        }),
        _ => None,
    }
}

/// Errors building a `CronViewSpec`. All have corresponding validator rules
/// (S005 in `validate_structural::validate_cron_view`) — runtime fallback.
#[derive(Debug, thiserror::Error)]
pub enum CronSpecError {
    /// Both `schedule` and `interval_seconds` set on a Cron view.
    #[error("schedule and interval_seconds are mutually exclusive")]
    ScheduleAndIntervalBothSet,
    /// Neither `schedule` nor `interval_seconds` set on a Cron view.
    #[error("Cron view requires one of schedule or interval_seconds")]
    NoSchedule,
    /// `interval_seconds` is < 1.
    #[error("interval_seconds must be >= 1")]
    IntervalTooSmall,
    /// `schedule` failed to parse as a cron expression.
    #[error("invalid cron schedule: {0}")]
    InvalidSchedule(String),
    /// Cron view's `handler.type` is not `codecomponent`.
    #[error("Cron view handler must be type=codecomponent")]
    HandlerNotCodecomponent,
}

// ── Dedupe (multi-instance, spec §5) ───────────────────────────────

/// Compute the StorageEngine namespace + key for per-tick dedupe.
fn dedupe_key(app_id: &str, view: &str, tick_epoch: i64) -> (&'static str, String) {
    ("cron", format!("{}:{}:{}", app_id, view, tick_epoch))
}

/// Compute the dedupe TTL: max(interval, 60s), capped at 3600s. Spec §4.3.
fn dedupe_ttl(schedule: &NextTick, now: DateTime<Utc>) -> std::time::Duration {
    use std::time::Duration;
    let base = match schedule {
        NextTick::Interval(d) => *d,
        NextTick::Cron(s) => {
            // Best-effort: gap between next two scheduled instants.
            let mut iter = s.after(&now);
            match (iter.next(), iter.next()) {
                (Some(t1), Some(t2)) => (t2 - t1).to_std().unwrap_or(Duration::from_secs(60)),
                _ => Duration::from_secs(60),
            }
        }
    };
    let secs = base.as_secs().max(60).min(3600);
    Duration::from_secs(secs)
}

/// Try to acquire the per-tick lock. Returns `Ok(true)` if this node won,
/// `Ok(false)` if another node already wrote, `Err` on storage failure.
async fn try_acquire_tick(
    storage: &dyn StorageEngine,
    app_id: &str,
    view: &str,
    tick_epoch: i64,
    node_id: &str,
    ttl: std::time::Duration,
) -> Result<bool, rivers_runtime::rivers_core::storage::StorageError> {
    let (ns, key) = dedupe_key(app_id, view, tick_epoch);
    let ttl_ms = ttl.as_millis() as u64;
    storage
        .set_if_absent(ns, &key, node_id.as_bytes().to_vec(), Some(ttl_ms))
        .await
}

// ── Metrics (spec §9.1) ────────────────────────────────────────────
//
// Uses the `metrics` crate facade (same pattern as `crates/riversd/src/
// server/metrics.rs`); metrics-exporter-prometheus registers them when
// the binary is built with the `metrics` feature. No explicit registration
// step needed.

mod cron_metrics {
    use metrics::{counter, histogram};

    pub fn record_run(app: &str, view: &str) {
        counter!("rivers_cron_runs_total",
            "app" => app.to_string(), "view" => view.to_string()).increment(1);
    }
    pub fn record_failure(app: &str, view: &str) {
        counter!("rivers_cron_failures_total",
            "app" => app.to_string(), "view" => view.to_string()).increment(1);
    }
    pub fn record_skipped_overlap(app: &str, view: &str) {
        counter!("rivers_cron_skipped_overlap_total",
            "app" => app.to_string(), "view" => view.to_string()).increment(1);
    }
    pub fn record_skipped_dedupe(app: &str, view: &str) {
        counter!("rivers_cron_skipped_dedupe_total",
            "app" => app.to_string(), "view" => view.to_string()).increment(1);
    }
    pub fn record_dropped_queue(app: &str, view: &str) {
        counter!("rivers_cron_dropped_queue_full_total",
            "app" => app.to_string(), "view" => view.to_string()).increment(1);
    }
    pub fn record_storage_error(app: &str, view: &str) {
        counter!("rivers_cron_storage_errors_total",
            "app" => app.to_string(), "view" => view.to_string()).increment(1);
    }
    pub fn observe_duration_ms(app: &str, view: &str, ms: f64) {
        histogram!("rivers_cron_duration_ms",
            "app" => app.to_string(), "view" => view.to_string()).record(ms);
    }
}

// ── Scheduler ──────────────────────────────────────────────────────

/// Walk a `LoadedBundle` and build a `CronViewSpec` for every Cron view.
/// Spec-build errors are logged and the offending view is skipped — one
/// bad view does not stop the others.
pub fn collect_cron_specs(bundle: &rivers_runtime::LoadedBundle) -> Vec<CronViewSpec> {
    let mut out = Vec::new();
    for app in &bundle.apps {
        let app_id = &app.manifest.app_id;
        for (view_name, view_cfg) in &app.config.api.views {
            if view_cfg.view_type != "Cron" {
                continue;
            }
            match CronViewSpec::from_view_config(app_id, view_name, view_cfg) {
                Ok(Some(spec)) => out.push(spec),
                Ok(None) => {} // not a Cron view (shouldn't happen given the filter above)
                Err(e) => {
                    tracing::error!(
                        target: "rivers.cron",
                        app = %app_id,
                        view = %view_name,
                        error = %e,
                        "Skipping Cron view — failed to build spec (validator should have caught this)"
                    );
                }
            }
        }
    }
    out
}

/// Public handle for the cron scheduler. Owns the per-view loops via
/// `tokio::task::JoinHandle`; shutdown cooperatively notifies all loops.
///
/// Wired into riversd's graceful-shutdown path at
/// `crates/riversd/src/server/lifecycle.rs` — after `axum::serve` returns
/// (no_ssl path) or after `wait_for_drain` (TLS path), the scheduler is
/// `take`n out of `AppContext::cron_scheduler` and `.shutdown().await`ed
/// before per-app logs flush.
pub struct CronScheduler {
    /// Handles to per-view loops. `JoinHandle::abort` is the fallback if a
    /// loop ignores the shutdown notify.
    handles: Vec<tokio::task::JoinHandle<()>>,
    /// Cooperative shutdown signal — every loop selects on this.
    shutdown: Arc<Notify>,
    /// For tests/observability: how many loops we successfully spawned.
    spawned_count: usize,
}

impl CronScheduler {
    /// Spawn one loop per Cron view in `specs`. Returns the scheduler handle.
    /// `node_id` is recorded as the dedupe-key value (diagnostic only).
    pub fn start(
        specs: Vec<CronViewSpec>,
        pool: Arc<ProcessPoolManager>,
        storage: Arc<dyn StorageEngine>,
        node_id: String,
    ) -> Self {
        // No registration step — `metrics` crate is a facade; the global
        // recorder (set up by `metrics-exporter-prometheus`) accepts any
        // counter/histogram name on first use.
        let shutdown = Arc::new(Notify::new());
        let mut handles = Vec::with_capacity(specs.len());
        let spawned_count = specs.len();

        for spec in specs {
            let pool = pool.clone();
            let storage = storage.clone();
            let shutdown = shutdown.clone();
            let node_id = node_id.clone();
            handles.push(tokio::spawn(async move {
                run_cron_loop(spec, pool, storage, node_id, shutdown).await;
            }));
        }

        tracing::info!(
            target: "rivers.cron",
            spawned = spawned_count,
            "Cron scheduler started"
        );

        CronScheduler {
            handles,
            shutdown,
            spawned_count,
        }
    }

    /// How many cron loops are running.
    pub fn spawned_count(&self) -> usize {
        self.spawned_count
    }

    /// Cooperatively stop all loops. Awaits each `JoinHandle`. Safe to call
    /// once per scheduler.
    pub async fn shutdown(self) {
        self.shutdown.notify_waiters();
        for h in self.handles {
            // Loops should exit promptly on the notify; abort as fallback.
            tokio::select! {
                _ = h => {}
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    tracing::warn!(
                        target: "rivers.cron",
                        "Cron loop did not exit within 5s of shutdown"
                    );
                }
            }
        }
    }
}

// ── Per-view runner ────────────────────────────────────────────────

/// Per-view cron loop. Runs until `shutdown.notified()` fires.
async fn run_cron_loop(
    spec: CronViewSpec,
    pool: Arc<ProcessPoolManager>,
    storage: Arc<dyn StorageEngine>,
    node_id: String,
    shutdown: Arc<Notify>,
) {
    // Per-view atomic flag for `OverlapPolicy::Skip`.
    let in_flight = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Per-view bounded queue for `OverlapPolicy::Queue`. The consumer task
    // dispatches sequentially; the loop's per-tick path just `try_send`s
    // onto the channel. Ticks dropped on full are metric'd.
    let queue_tx = if spec.overlap == OverlapPolicy::Queue {
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<TickJob>(spec.max_concurrent.max(1) as usize);
        let pool = pool.clone();
        let storage = storage.clone();
        let app_id = spec.app_id.clone();
        let view = spec.view_name.clone();
        let entry = spec.entrypoint.clone();
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                dispatch_tick(&pool, storage.clone(), &app_id, &view, &entry, job).await;
            }
        });
        Some(tx)
    } else {
        None
    };

    tracing::debug!(
        target: "rivers.cron",
        app = %spec.app_id,
        view = %spec.view_name,
        "Cron loop started"
    );

    loop {
        let now = Utc::now();
        let next = match spec.schedule.next_after(now) {
            Some(t) => t,
            None => {
                // Schedule exhausted — exit loop quietly.
                tracing::warn!(
                    target: "rivers.cron",
                    app = %spec.app_id,
                    view = %spec.view_name,
                    "Cron schedule produced no future tick — loop exiting"
                );
                return;
            }
        };

        // Sleep until next tick OR shutdown, whichever fires first.
        let until_tick = (next - now).to_std().unwrap_or(std::time::Duration::ZERO);
        tokio::select! {
            _ = shutdown.notified() => {
                tracing::debug!(
                    target: "rivers.cron",
                    app = %spec.app_id,
                    view = %spec.view_name,
                    "Cron loop shutting down"
                );
                return;
            }
            _ = tokio::time::sleep(until_tick) => {}
        }

        let tick_epoch = next.timestamp();
        let ttl = dedupe_ttl(&spec.schedule, next);

        // Multi-instance dedupe.
        match try_acquire_tick(
            storage.as_ref(),
            &spec.app_id,
            &spec.view_name,
            tick_epoch,
            &node_id,
            ttl,
        )
        .await
        {
            Ok(true) => {} // we own this tick
            Ok(false) => {
                cron_metrics::record_skipped_dedupe(&spec.app_id, &spec.view_name);
                tracing::debug!(
                    target: "rivers.cron",
                    app = %spec.app_id,
                    view = %spec.view_name,
                    tick_epoch,
                    "Cron tick skipped — another node won the lock"
                );
                continue;
            }
            Err(e) => {
                cron_metrics::record_storage_error(&spec.app_id, &spec.view_name);
                tracing::warn!(
                    target: "rivers.cron",
                    app = %spec.app_id,
                    view = %spec.view_name,
                    error = %e,
                    "Cron tick storage error — skipping"
                );
                continue;
            }
        }

        let job = TickJob {
            scheduled: next,
            fired: Utc::now(),
            tick_epoch,
            node_id: node_id.clone(),
            in_flight: in_flight.clone(),
        };

        match spec.overlap {
            OverlapPolicy::Skip => {
                if in_flight
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::SeqCst,
                        std::sync::atomic::Ordering::SeqCst,
                    )
                    .is_err()
                {
                    cron_metrics::record_skipped_overlap(&spec.app_id, &spec.view_name);
                    tracing::debug!(
                        target: "rivers.cron",
                        app = %spec.app_id,
                        view = %spec.view_name,
                        tick_epoch,
                        "Cron tick skipped — previous tick still running (overlap=skip)"
                    );
                    continue;
                }
                let pool = pool.clone();
                let storage = storage.clone();
                let app_id = spec.app_id.clone();
                let view = spec.view_name.clone();
                let entry = spec.entrypoint.clone();
                tokio::spawn(async move {
                    dispatch_tick(&pool, storage, &app_id, &view, &entry, job).await;
                });
            }
            OverlapPolicy::Queue => {
                let tx = queue_tx
                    .as_ref()
                    .expect("queue_tx present iff overlap=queue");
                if let Err(_e) = tx.try_send(job) {
                    cron_metrics::record_dropped_queue(&spec.app_id, &spec.view_name);
                    tracing::warn!(
                        target: "rivers.cron",
                        app = %spec.app_id,
                        view = %spec.view_name,
                        tick_epoch,
                        "Cron tick dropped — queue full (overlap=queue)"
                    );
                }
            }
            OverlapPolicy::Allow => {
                let pool = pool.clone();
                let storage = storage.clone();
                let app_id = spec.app_id.clone();
                let view = spec.view_name.clone();
                let entry = spec.entrypoint.clone();
                tokio::spawn(async move {
                    dispatch_tick(&pool, storage, &app_id, &view, &entry, job).await;
                });
            }
        }
    }
}

/// One tick's worth of state — captured at tick-fire time and threaded into
/// the dispatch path so `ctx.cron` carries scheduling metadata.
#[derive(Debug, Clone)]
struct TickJob {
    scheduled: DateTime<Utc>,
    fired: DateTime<Utc>,
    tick_epoch: i64,
    node_id: String,
    /// Cleared in `dispatch_tick`'s `finally` when overlap=skip — only used
    /// for `Skip`; `Queue`/`Allow` ignore it.
    in_flight: Arc<std::sync::atomic::AtomicBool>,
}

/// Build the synthetic args envelope and dispatch via ProcessPool.
/// `storage` is attached to the TaskContext so `ctx.store.*` callbacks
/// work in the handler — same backend the scheduler uses for dedupe.
async fn dispatch_tick(
    pool: &ProcessPoolManager,
    storage: Arc<dyn StorageEngine>,
    app_id: &str,
    view_name: &str,
    entrypoint: &Entrypoint,
    job: TickJob,
) {
    let start = std::time::Instant::now();
    cron_metrics::record_run(app_id, view_name);

    // Synthetic dispatch envelope — spec §4.4.
    let args = serde_json::json!({
        "request": {
            "headers":     {},
            "body":        serde_json::Value::Null,
            "path_params": {},
            "query":       {},
        },
        "session":     serde_json::Value::Null,
        "path_params": {},
        "cron": {
            "view_name":  view_name,
            "tick_epoch": job.tick_epoch,
            "scheduled":  job.scheduled.to_rfc3339(),
            "fired":      job.fired.to_rfc3339(),
            "node_id":    job.node_id,
        },
    });

    let trace_id = format!("cron:{}:{}:{}", app_id, view_name, job.tick_epoch);
    let builder = TaskContextBuilder::new()
        .entrypoint(entrypoint.clone())
        .args(args)
        .trace_id(trace_id)
        .storage(storage);
    let builder = crate::task_enrichment::enrich(builder, app_id, TaskKind::Rest);

    let result = match builder.build() {
        Ok(ctx) => pool.dispatch("default", ctx).await,
        Err(e) => {
            tracing::error!(
                target: "rivers.cron",
                app = %app_id,
                view = %view_name,
                tick_epoch = job.tick_epoch,
                error = %e,
                "Cron tick build error"
            );
            cron_metrics::record_failure(app_id, view_name);
            // Clear in_flight even on build failure (overlap=skip).
            job.in_flight
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let dispatch_latency_ms = (job.fired - job.scheduled).num_milliseconds();
    cron_metrics::observe_duration_ms(app_id, view_name, elapsed_ms as f64);

    match result {
        Ok(_) => {
            tracing::debug!(
                target: "rivers.cron",
                app = %app_id,
                view = %view_name,
                tick_epoch = job.tick_epoch,
                duration_ms = elapsed_ms,
                dispatch_latency_ms,
                "Cron tick OK"
            );
        }
        Err(e) => {
            cron_metrics::record_failure(app_id, view_name);
            tracing::error!(
                target: "rivers.cron",
                app = %app_id,
                view = %view_name,
                tick_epoch = job.tick_epoch,
                duration_ms = elapsed_ms,
                error = %e,
                "Cron tick failed"
            );
        }
    }

    // Always clear the in_flight flag — overlap=skip needs it; the others
    // just don't read it.
    job.in_flight
        .store(false, std::sync::atomic::Ordering::SeqCst);
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_tick_interval_returns_now_plus_duration() {
        let s = NextTick::Interval(std::time::Duration::from_secs(60));
        let now = Utc::now();
        let next = s.next_after(now).unwrap();
        let delta = (next - now).num_seconds();
        assert_eq!(delta, 60, "expected 60s ahead, got {}s", delta);
    }

    #[test]
    fn next_tick_cron_returns_future_instant() {
        let s = NextTick::Cron(
            cron::Schedule::from_str("0 */5 * * * *").unwrap(),
        );
        let now = Utc::now();
        let next = s.next_after(now).unwrap();
        assert!(next > now, "next must be in the future");
        // Within 5 minutes (next 5-minute boundary).
        assert!(
            (next - now).num_seconds() <= 5 * 60,
            "expected next within 5 min"
        );
    }

    #[test]
    fn overlap_policy_default_is_skip() {
        assert_eq!(
            OverlapPolicy::from_str_or_default(None),
            OverlapPolicy::Skip
        );
        assert_eq!(
            OverlapPolicy::from_str_or_default(Some("skip")),
            OverlapPolicy::Skip
        );
        assert_eq!(
            OverlapPolicy::from_str_or_default(Some("queue")),
            OverlapPolicy::Queue
        );
        assert_eq!(
            OverlapPolicy::from_str_or_default(Some("allow")),
            OverlapPolicy::Allow
        );
        // Unknown values defensive-default to Skip.
        assert_eq!(
            OverlapPolicy::from_str_or_default(Some("bogus")),
            OverlapPolicy::Skip
        );
    }

    #[test]
    fn dedupe_ttl_min_60s_max_3600s() {
        // 1s interval clamps up to 60s.
        let ttl = dedupe_ttl(
            &NextTick::Interval(std::time::Duration::from_secs(1)),
            Utc::now(),
        );
        assert_eq!(ttl.as_secs(), 60);

        // 7200s interval clamps down to 3600s.
        let ttl = dedupe_ttl(
            &NextTick::Interval(std::time::Duration::from_secs(7200)),
            Utc::now(),
        );
        assert_eq!(ttl.as_secs(), 3600);

        // 300s interval is in range.
        let ttl = dedupe_ttl(
            &NextTick::Interval(std::time::Duration::from_secs(300)),
            Utc::now(),
        );
        assert_eq!(ttl.as_secs(), 300);
    }

    #[test]
    fn dedupe_key_is_namespaced() {
        let (ns, key) = dedupe_key("app1", "recompute", 1715299200);
        assert_eq!(ns, "cron");
        assert_eq!(key, "app1:recompute:1715299200");
    }

    #[tokio::test]
    async fn try_acquire_tick_first_caller_wins() {
        // Multi-instance dedupe primitive: two calls against the same
        // StorageEngine for the same (app, view, tick_epoch) tuple — first
        // returns Ok(true), second returns Ok(false). This is the
        // "exactly-once-per-tick" guarantee per spec §5.
        use rivers_runtime::rivers_core::storage::InMemoryStorageEngine;
        let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
        let ttl = std::time::Duration::from_secs(60);

        let won_a = try_acquire_tick(
            storage.as_ref(),
            "app1",
            "recompute",
            1715299200,
            "node-A",
            ttl,
        )
        .await
        .unwrap();
        assert!(won_a, "first node should win the tick");

        let won_b = try_acquire_tick(
            storage.as_ref(),
            "app1",
            "recompute",
            1715299200,
            "node-B",
            ttl,
        )
        .await
        .unwrap();
        assert!(!won_b, "second node should not win the same tick");

        // Different tick_epoch — both can win.
        let won_a_next = try_acquire_tick(
            storage.as_ref(),
            "app1",
            "recompute",
            1715299260,
            "node-A",
            ttl,
        )
        .await
        .unwrap();
        assert!(won_a_next, "node-A should win the *next* tick");
    }

    #[tokio::test]
    async fn try_acquire_tick_isolates_views_and_apps() {
        // Different (app, view) pairs use different keys — they don't
        // dedupe each other.
        use rivers_runtime::rivers_core::storage::InMemoryStorageEngine;
        let storage: Arc<dyn StorageEngine> = Arc::new(InMemoryStorageEngine::new());
        let ttl = std::time::Duration::from_secs(60);
        let epoch = 1715299200;

        assert!(try_acquire_tick(storage.as_ref(), "appA", "view1", epoch, "n", ttl).await.unwrap());
        // Same app, different view — independent.
        assert!(try_acquire_tick(storage.as_ref(), "appA", "view2", epoch, "n", ttl).await.unwrap());
        // Different app, same view name — independent.
        assert!(try_acquire_tick(storage.as_ref(), "appB", "view1", epoch, "n", ttl).await.unwrap());
        // Same app+view+epoch — duplicate, blocked.
        assert!(!try_acquire_tick(storage.as_ref(), "appA", "view1", epoch, "n", ttl).await.unwrap());
    }

    #[test]
    fn cron_view_spec_returns_none_for_non_cron_view() {
        // Build a Rest view config — should yield Ok(None).
        // We only need view_type and handler set; default rest of the fields.
        let cfg = make_rest_view_config();
        let spec = CronViewSpec::from_view_config("app1", "v", &cfg).unwrap();
        assert!(spec.is_none());
    }

    #[test]
    fn cron_view_spec_rejects_handler_dataview() {
        // A Cron view with handler.type=dataview should error — codecomponent only.
        let mut cfg = make_cron_view_config(NextTick::Interval(std::time::Duration::from_secs(60)));
        cfg.handler = rivers_runtime::view::HandlerConfig::Dataview {
            dataview: "irrelevant".to_string(),
        };
        let err = CronViewSpec::from_view_config("app1", "v", &cfg).unwrap_err();
        assert!(matches!(err, CronSpecError::HandlerNotCodecomponent));
    }

    #[test]
    fn cron_view_spec_rejects_neither_schedule_nor_interval() {
        let mut cfg = make_cron_view_config_skeleton();
        cfg.schedule = None;
        cfg.interval_seconds = None;
        let err = CronViewSpec::from_view_config("app1", "v", &cfg).unwrap_err();
        assert!(matches!(err, CronSpecError::NoSchedule));
    }

    #[test]
    fn cron_view_spec_rejects_both_schedule_and_interval() {
        let mut cfg = make_cron_view_config_skeleton();
        cfg.schedule = Some("0 */5 * * * *".to_string());
        cfg.interval_seconds = Some(300);
        let err = CronViewSpec::from_view_config("app1", "v", &cfg).unwrap_err();
        assert!(matches!(err, CronSpecError::ScheduleAndIntervalBothSet));
    }

    #[test]
    fn cron_view_spec_parses_canonical() {
        let cfg = make_cron_view_config(NextTick::Interval(std::time::Duration::from_secs(300)));
        let spec = CronViewSpec::from_view_config("app1", "v", &cfg)
            .unwrap()
            .unwrap();
        assert_eq!(spec.app_id, "app1");
        assert_eq!(spec.view_name, "v");
        assert_eq!(spec.overlap, OverlapPolicy::Skip);
        assert_eq!(spec.max_concurrent, 16);
    }

    // ── Test helpers ──────────────────────────────────────────────

    fn make_rest_view_config() -> ApiViewConfig {
        ApiViewConfig {
            view_type: "Rest".to_string(),
            path: Some("/foo".to_string()),
            method: Some("GET".to_string()),
            handler: rivers_runtime::view::HandlerConfig::Dataview {
                dataview: "ds".to_string(),
            },
            parameter_mapping: None,
            dataviews: vec![],
            primary: None,
            streaming: None,
            streaming_format: None,
            stream_timeout_ms: None,
            guard: false,
            auth: None,
            guard_config: None,
            guard_view: None,
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
            tools: Default::default(),
            resources: Default::default(),
            prompts: Default::default(),
            instructions: None,
            session: None,
            federation: vec![],
            response_headers: None,
            schedule: None,
            interval_seconds: None,
            overlap_policy: None,
            max_concurrent: None,
        }
    }

    fn make_cron_view_config_skeleton() -> ApiViewConfig {
        let mut cfg = make_rest_view_config();
        cfg.view_type = "Cron".to_string();
        cfg.path = None;
        cfg.method = None;
        cfg.handler = rivers_runtime::view::HandlerConfig::Codecomponent {
            language: "javascript".to_string(),
            module: "handlers/cron.ts".to_string(),
            entrypoint: "tick".to_string(),
            resources: vec![],
        };
        cfg
    }

    fn make_cron_view_config(schedule: NextTick) -> ApiViewConfig {
        let mut cfg = make_cron_view_config_skeleton();
        match schedule {
            NextTick::Cron(s) => cfg.schedule = Some(s.source().to_string()),
            NextTick::Interval(d) => cfg.interval_seconds = Some(d.as_secs()),
        }
        cfg
    }
}
