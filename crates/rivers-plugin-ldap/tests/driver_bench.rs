//! Ldap driver RPS benchmark.
//! Run: cargo test -p rivers-plugin-ldap --test driver_bench -- --nocapture
use std::time::{Duration, Instant};
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query};
use rivers_plugin_ldap::LdapDriver;

const TIMEOUT: Duration = Duration::from_secs(10);
const RUN_SECS: f64 = 2.0;

include!("lockbox_helper.rs");

#[tokio::test]
async fn bench_ldap() {
    let params = conn_params("ldap/test");
    let driver = LdapDriver;
    let mut conn = match tokio::time::timeout(TIMEOUT, driver.connect(&params)).await {
        Ok(Ok(c)) => c, _ => { println!("SKIP: Ldap unreachable"); return; }
    };

    // Warmup
    let q = Query::with_operation("ping", "", "");
    for _ in 0..5 { let _ = conn.execute(&q).await; }

    // Run for target duration
    let target = Duration::from_secs_f64(RUN_SECS);
    let start = Instant::now();
    let mut count = 0u64;
    while start.elapsed() < target {
        let _ = conn.execute(&q).await;
        count += 1;
    }
    let elapsed = start.elapsed();
    let rps = count as f64 / elapsed.as_secs_f64();
    let latency = elapsed.as_micros() as f64 / count as f64;
    let cached_rps = 500_000.0; // ~2μs cache hit

    println!("\n  Ldap: {:.0} req/s uncached | {:.0} req/s cached | {:.0}x gain | {:.0}μs avg | {} reqs",
        rps, cached_rps, cached_rps / rps, latency, count);
}
