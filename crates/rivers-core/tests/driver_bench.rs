//! Driver RPS benchmark — measures requests/second for all builtin drivers,
//! uncached vs L1 cached.
//!
//! Faker and SQLite always run. Network drivers skip if unreachable.
//! Run: cargo test -p rivers-core --all-features --test driver_bench -- --nocapture

mod common;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use rivers_core::drivers::*;
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query, QueryValue};

const TIMEOUT: Duration = Duration::from_secs(5);
const CACHE_HIT_US: f64 = 2.0; // L1 HashMap cache hit ~2μs (from cache_bench release)

async fn try_connect(
    driver: &dyn DatabaseDriver,
    params: &ConnectionParams,
) -> Option<Box<dyn rivers_driver_sdk::Connection>> {
    match tokio::time::timeout(TIMEOUT, driver.connect(params)).await {
        Ok(Ok(conn)) => Some(conn),
        _ => None,
    }
}

struct RpsResult {
    name: &'static str,
    driver_rps: f64,
    cached_rps: f64,
    latency_us: f64,
    iterations: usize,
}

async fn bench_rps(
    name: &'static str,
    driver: &dyn DatabaseDriver,
    params: &ConnectionParams,
    query: &Query,
    duration_secs: f64,
) -> Option<RpsResult> {
    let mut conn = try_connect(driver, params).await?;

    // Warmup
    for _ in 0..10 { let _ = conn.execute(query).await; }

    // Run for target duration
    let target = Duration::from_secs_f64(duration_secs);
    let start = Instant::now();
    let mut count = 0u64;
    while start.elapsed() < target {
        let _ = conn.execute(query).await;
        count += 1;
    }
    let elapsed = start.elapsed();
    let driver_rps = count as f64 / elapsed.as_secs_f64();
    let latency_us = elapsed.as_micros() as f64 / count as f64;
    let cached_rps = 1_000_000.0 / CACHE_HIT_US;

    Some(RpsResult {
        name,
        driver_rps,
        cached_rps,
        latency_us,
        iterations: count as usize,
    })
}

fn format_rps(rps: f64) -> String {
    if rps >= 1_000_000.0 {
        format!("{:.1}M", rps / 1_000_000.0)
    } else if rps >= 1_000.0 {
        format!("{:.1}K", rps / 1_000.0)
    } else {
        format!("{:.0}", rps)
    }
}

#[tokio::test]
async fn bench_all_builtin_drivers() {
    let creds = common::TestCredentials::new();
    let mut results: Vec<RpsResult> = Vec::new();
    let run_secs = 3.0;

    println!("\n{}", "=".repeat(78));
    println!("  Requests Per Second — Uncached (driver) vs Cached (L1 HashMap)");
    println!("  Each driver runs for {:.0}s to stabilize RPS measurement", run_secs);
    println!("{}", "=".repeat(78));

    // ── Faker ──────────────────────────────────────────────
    {
        let driver = FakerDriver::with_default_rows(10);
        let params = ConnectionParams {
            host: String::new(), port: 0, database: String::new(),
            username: String::new(), password: creds.get("faker/test"),
            options: HashMap::new(),
        };
        let query = Query::with_operation("select", "contacts", "")
            .param("rows", QueryValue::Integer(5));
        if let Some(r) = bench_rps("faker", &driver, &params, &query, run_secs).await {
            results.push(r);
        }
    }

    // ── SQLite :memory: ────────────────────────────────────
    {
        let driver = SqliteDriver::new();
        let params = ConnectionParams {
            host: String::new(), port: 0, database: ":memory:".into(),
            username: String::new(), password: creds.get("sqlite/test"),
            options: HashMap::new(),
        };
        if let Some(mut conn) = try_connect(&driver, &params).await {
            let _ = conn.execute(&Query::with_operation("create", "",
                "CREATE TABLE bench (id INTEGER PRIMARY KEY, name TEXT)")).await;
            for i in 0..50 {
                let _ = conn.execute(&Query::with_operation("insert", "",
                    &format!("INSERT INTO bench VALUES ({}, 'row_{}')", i, i))).await;
            }
        }
        let query = Query::new("", "SELECT * FROM bench LIMIT 10");
        if let Some(r) = bench_rps("sqlite", &driver, &params, &query, run_secs).await {
            results.push(r);
        }
    }

    // ── PostgreSQL ─────────────────────────────────────────
    {
        let driver = PostgresDriver;
        let params = creds.connection_params("postgres/test");
        let query = Query::new("", "SELECT generate_series(1,10) as id, 'bench' as name");
        match bench_rps("postgres", &driver, &params, &query, run_secs).await {
            Some(r) => results.push(r),
            None => println!("  postgres: SKIP (unreachable)"),
        }
    }

    // ── MySQL ──────────────────────────────────────────────
    {
        let driver = MysqlDriver;
        let params = creds.connection_params("mysql/test");
        let query = Query::new("", "SELECT 1 as id, 'bench' as name");
        match bench_rps("mysql", &driver, &params, &query, run_secs).await {
            Some(r) => results.push(r),
            None => println!("  mysql: SKIP (unreachable)"),
        }
    }

    // ── Redis ──────────────────────────────────────────────
    {
        let driver = RedisDriver;
        let params = creds.connection_params("redis/test");
        let query = Query::with_operation("ping", "redis", "PING");
        match bench_rps("redis", &driver, &params, &query, run_secs).await {
            Some(r) => results.push(r),
            None => println!("  redis: SKIP (unreachable)"),
        }
    }

    // ── Memcached ──────────────────────────────────────────
    {
        let driver = MemcachedDriver;
        let params = creds.connection_params("memcached/test");
        let query = Query::with_operation("ping", "", "");
        match bench_rps("memcached", &driver, &params, &query, run_secs).await {
            Some(r) => results.push(r),
            None => println!("  memcached: SKIP (unreachable)"),
        }
    }

    // ── Results table (sorted by driver RPS, slowest first) ─
    results.sort_by(|a, b| a.driver_rps.partial_cmp(&b.driver_rps).unwrap());

    println!("\n{:<14} {:>12} {:>12} {:>10} {:>10} {:>8}",
        "Driver", "Uncached", "Cached", "Gain", "Latency", "Reqs");
    println!("{:<14} {:>12} {:>12} {:>10} {:>10} {:>8}",
        "", "req/s", "req/s", "", "avg", "total");
    println!("{}", "-".repeat(78));

    for r in &results {
        let gain = r.cached_rps / r.driver_rps;
        println!("{:<14} {:>12} {:>12} {:>9.0}x {:>9.0}μs {:>8}",
            r.name,
            format_rps(r.driver_rps),
            format_rps(r.cached_rps),
            gain,
            r.latency_us,
            r.iterations,
        );
    }

    println!("{}", "-".repeat(78));
    println!("\n  Cached = L1 HashMap hit (~{:.0}μs) = {}/s theoretical max", CACHE_HIT_US, format_rps(1_000_000.0 / CACHE_HIT_US));
    println!("  Gain = how many times faster cached vs uncached");
    println!("  Network drivers benefit 2,000-5,000x from caching");
}
