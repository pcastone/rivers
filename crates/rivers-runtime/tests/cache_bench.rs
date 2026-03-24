//! Cache performance benchmark — full DataView execution path through multiple drivers.
//!
//! Tests: Faker, SQLite (:memory:), L1 cache, L2 InMemory-backed cache
//! PostgreSQL requires live infra — skipped if unreachable.
//!
//! Run: cargo test -p rivers-runtime --features full --test cache_bench -- --nocapture

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rivers_driver_sdk::types::{QueryResult, QueryValue};
use rivers_runtime::tiered_cache::{
    DataViewCache, DataViewCachingPolicy, LruDataViewCache, TieredDataViewCache,
};

fn sample_result(id: i64) -> QueryResult {
    QueryResult {
        rows: vec![[
            ("id".to_string(), QueryValue::Integer(id)),
            ("name".to_string(), QueryValue::String(format!("User {}", id))),
            ("email".to_string(), QueryValue::String(format!("user{}@example.com", id))),
        ]
        .into_iter()
        .collect()],
        affected_rows: 1,
        last_insert_id: None,
    }
}

fn make_params(id: i64) -> HashMap<String, QueryValue> {
    [("id".to_string(), QueryValue::Integer(id))].into_iter().collect()
}

// ── Old VecDeque LRU (for comparison) ────────────────────────────

mod old_cache {
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};
    use rivers_driver_sdk::types::QueryResult;
    use tokio::sync::Mutex;

    struct CachedEntry {
        result: QueryResult,
        expires_at: Instant,
    }

    pub struct OldLruCache {
        entries: Mutex<VecDeque<(String, CachedEntry)>>,
        max_entries: usize,
        ttl: Duration,
    }

    impl OldLruCache {
        pub fn new(max_entries: usize, ttl_seconds: u64) -> Self {
            Self {
                entries: Mutex::new(VecDeque::with_capacity(max_entries)),
                max_entries,
                ttl: Duration::from_secs(ttl_seconds),
            }
        }

        pub async fn get(&self, key: &str) -> Option<QueryResult> {
            let mut entries = self.entries.lock().await;
            let now = Instant::now();
            if let Some(pos) = entries.iter().position(|(k, _)| k == key) {
                let (_, entry) = &entries[pos];
                if now >= entry.expires_at {
                    entries.remove(pos);
                    return None;
                }
                let item = entries.remove(pos).unwrap();
                let result = item.1.result.clone();
                entries.push_back(item);
                Some(result)
            } else {
                None
            }
        }

        pub async fn set(&self, key: String, result: QueryResult) {
            let mut entries = self.entries.lock().await;
            let now = Instant::now();
            if let Some(pos) = entries.iter().position(|(k, _)| k == &key) {
                entries.remove(pos);
            }
            while entries.len() >= self.max_entries {
                entries.pop_front();
            }
            entries.push_back((key, CachedEntry {
                result,
                expires_at: now + self.ttl,
            }));
        }
    }
}

// ── 1. L1 LRU: Old vs New ───────────────────────────────────────

#[tokio::test]
async fn bench_1_l1_lru_old_vs_new() {
    let sizes = [100, 500, 1000];
    let lookups = 10_000;

    println!("\n{}", "=".repeat(76));
    println!("  L1 LRU: Old (VecDeque O(n)) vs New (HashMap O(1))");
    println!("{}\n", "=".repeat(76));
    println!("{:<8} {:>12} {:>12} {:>8}  {:>12} {:>12} {:>8}",
        "Entries", "Old Hit", "New Hit", "Speedup", "Old Miss", "New Miss", "Speedup");
    println!("{}", "-".repeat(76));

    for &size in &sizes {
        // OLD
        let old = old_cache::OldLruCache::new(size, 300);
        for i in 0..size as i64 {
            old.set(format!("k:{}", i), sample_result(i)).await;
        }
        let start = Instant::now();
        for _ in 0..lookups {
            let _ = old.get("k:0").await;
        }
        let old_hit = start.elapsed();

        let start = Instant::now();
        for i in 0..lookups {
            let _ = old.get(&format!("miss:{}", i)).await;
        }
        let old_miss = start.elapsed();

        // NEW
        let new_c = LruDataViewCache::new(usize::MAX, size, 300);
        for i in 0..size as i64 {
            new_c.set(format!("k:{}", i), Arc::new(sample_result(i)), None).await;
        }
        let start = Instant::now();
        for _ in 0..lookups {
            let _ = new_c.get("k:0").await;
        }
        let new_hit = start.elapsed();

        let start = Instant::now();
        for i in 0..lookups {
            let _ = new_c.get(&format!("miss:{}", i)).await;
        }
        let new_miss = start.elapsed();

        println!("{:<8} {:>12.2?} {:>12.2?} {:>7.1}x  {:>12.2?} {:>12.2?} {:>7.1}x",
            size,
            old_hit, new_hit,
            old_hit.as_nanos() as f64 / new_hit.as_nanos() as f64,
            old_miss, new_miss,
            old_miss.as_nanos() as f64 / new_miss.as_nanos() as f64,
        );
    }
}

// ── 2. Faker driver: uncached vs L1 cached ───────────────────────

#[tokio::test]
async fn bench_2_faker_cached_vs_uncached() {
    use rivers_core::DriverFactory;
    use rivers_core::drivers::FakerDriver;
    use rivers_driver_sdk::ConnectionParams;
    use rivers_runtime::dataview::{DataViewConfig, DataViewCachingConfig};
    use rivers_runtime::dataview_engine::DataViewRegistry;
    use rivers_runtime::tiered_cache::NoopDataViewCache;

    let iterations = 5_000;

    println!("\n{}", "=".repeat(70));
    println!("  Faker Driver: Uncached vs L1 Cached ({} iterations)", iterations);
    println!("{}\n", "=".repeat(70));

    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(FakerDriver::with_default_rows(10)));
    let factory = Arc::new(factory);

    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "faker".to_string());
    let mut ds_params = HashMap::new();
    ds_params.insert("faker-ds".to_string(), ConnectionParams {
        host: String::new(), port: 0, database: String::new(),
        username: String::new(), password: String::new(), options: opts,
    });
    let ds_params = Arc::new(ds_params);

    let make_config = |caching: Option<DataViewCachingConfig>| DataViewConfig {
        name: "list_contacts".into(), datasource: "faker-ds".into(),
        query: Some("schemas/contact.schema.json".into()), caching,
        parameters: vec![], return_schema: None, invalidates: vec![],
        validate_result: false, strict_parameters: false,
        get_query: None, post_query: None, put_query: None, delete_query: None,
        get_schema: None, post_schema: None, put_schema: None, delete_schema: None,
        get_parameters: vec![], post_parameters: vec![],
        put_parameters: vec![], delete_parameters: vec![], streaming: false,
    };

    // Uncached
    let mut registry = DataViewRegistry::new();
    registry.register(make_config(None));
    let executor = rivers_runtime::DataViewExecutor::new(
        registry, factory.clone(), ds_params.clone(), Arc::new(NoopDataViewCache),
    );
    let start = Instant::now();
    for i in 0..iterations {
        let _ = executor.execute("list_contacts", HashMap::new(), "GET", &format!("t-{}", i)).await;
    }
    let uncached = start.elapsed();

    // L1 cached
    let mut registry = DataViewRegistry::new();
    registry.register(make_config(Some(DataViewCachingConfig {
        ttl_seconds: 300, l1_enabled: true, l1_max_bytes: usize::MAX,
        l1_max_entries: 100_000, l2_enabled: false, l2_max_value_bytes: 131_072,
    })));
    let cache = Arc::new(TieredDataViewCache::new(DataViewCachingPolicy {
        ttl_seconds: 300, ..Default::default()
    }));
    let executor = rivers_runtime::DataViewExecutor::new(
        registry, factory.clone(), ds_params.clone(), cache,
    );
    let _ = executor.execute("list_contacts", HashMap::new(), "GET", "warm").await;
    let start = Instant::now();
    for i in 0..iterations {
        let _ = executor.execute("list_contacts", HashMap::new(), "GET", &format!("t-{}", i)).await;
    }
    let cached = start.elapsed();

    println!("  Uncached (driver every time): {:>10.2?}  ({:.0} ops/s)",
        uncached, iterations as f64 / uncached.as_secs_f64());
    println!("  L1 Cached (hit path):         {:>10.2?}  ({:.0} ops/s)",
        cached, iterations as f64 / cached.as_secs_f64());
    println!("  Speedup:                      {:>10.1}x",
        uncached.as_nanos() as f64 / cached.as_nanos() as f64);
}

// ── 3. SQLite driver: uncached vs L1 cached ──────────────────────

#[tokio::test]
async fn bench_3_sqlite_cached_vs_uncached() {
    use rivers_core::DriverFactory;
    use rivers_core::drivers::SqliteDriver;
    use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query};
    use rivers_runtime::dataview::{DataViewConfig, DataViewCachingConfig};
    use rivers_runtime::dataview_engine::DataViewRegistry;
    use rivers_runtime::tiered_cache::NoopDataViewCache;

    let iterations = 2_000;

    println!("\n{}", "=".repeat(70));
    println!("  SQLite (:memory:): Uncached vs L1 Cached ({} iters)", iterations);
    println!("{}\n", "=".repeat(70));

    // Seed a temp SQLite DB
    let sqlite = SqliteDriver::new();
    let params = ConnectionParams {
        host: String::new(), port: 0, database: ":memory:".into(),
        username: String::new(), password: String::new(), options: HashMap::new(),
    };
    let mut conn = DatabaseDriver::connect(&sqlite, &params).await.unwrap();
    conn.execute(&Query::with_operation("create", "",
        "CREATE TABLE contacts (id INTEGER PRIMARY KEY, name TEXT, email TEXT)")).await.unwrap();
    for i in 0..100 {
        conn.execute(&Query::with_operation("insert", "",
            &format!("INSERT INTO contacts VALUES ({}, 'User {}', 'u{}@test.com')", i, i, i))).await.unwrap();
    }
    drop(conn);

    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(sqlite));
    let factory = Arc::new(factory);

    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "sqlite".to_string());
    let mut ds_params = HashMap::new();
    ds_params.insert("sqlite-ds".to_string(), ConnectionParams {
        host: String::new(), port: 0, database: ":memory:".into(),
        username: String::new(), password: String::new(), options: opts,
    });
    let ds_params = Arc::new(ds_params);

    let make_config = |caching: Option<DataViewCachingConfig>| DataViewConfig {
        name: "list_contacts".into(), datasource: "sqlite-ds".into(),
        query: Some("SELECT * FROM contacts LIMIT 10".into()), caching,
        parameters: vec![], return_schema: None, invalidates: vec![],
        validate_result: false, strict_parameters: false,
        get_query: None, post_query: None, put_query: None, delete_query: None,
        get_schema: None, post_schema: None, put_schema: None, delete_schema: None,
        get_parameters: vec![], post_parameters: vec![],
        put_parameters: vec![], delete_parameters: vec![], streaming: false,
    };

    // Uncached
    let mut registry = DataViewRegistry::new();
    registry.register(make_config(None));
    let executor = rivers_runtime::DataViewExecutor::new(
        registry, factory.clone(), ds_params.clone(), Arc::new(NoopDataViewCache),
    );
    let start = Instant::now();
    for i in 0..iterations {
        let _ = executor.execute("list_contacts", HashMap::new(), "GET", &format!("t-{}", i)).await;
    }
    let uncached = start.elapsed();

    // L1 cached
    let mut registry = DataViewRegistry::new();
    registry.register(make_config(Some(DataViewCachingConfig {
        ttl_seconds: 300, l1_enabled: true, l1_max_bytes: usize::MAX,
        l1_max_entries: 100_000, l2_enabled: false, l2_max_value_bytes: 131_072,
    })));
    let cache = Arc::new(TieredDataViewCache::new(DataViewCachingPolicy {
        ttl_seconds: 300, ..Default::default()
    }));
    let executor = rivers_runtime::DataViewExecutor::new(
        registry, factory.clone(), ds_params.clone(), cache,
    );
    let _ = executor.execute("list_contacts", HashMap::new(), "GET", "warm").await;
    let start = Instant::now();
    for i in 0..iterations {
        let _ = executor.execute("list_contacts", HashMap::new(), "GET", &format!("t-{}", i)).await;
    }
    let cached = start.elapsed();

    println!("  Uncached (SQLite every time): {:>10.2?}  ({:.0} ops/s)",
        uncached, iterations as f64 / uncached.as_secs_f64());
    println!("  L1 Cached (hit path):         {:>10.2?}  ({:.0} ops/s)",
        cached, iterations as f64 / cached.as_secs_f64());
    println!("  Speedup:                      {:>10.1}x",
        uncached.as_nanos() as f64 / cached.as_nanos() as f64);
}

// ── 4. L2 InMemory StorageEngine backend ─────────────────────────

#[tokio::test]
async fn bench_4_l2_inmemory_cache() {
    use rivers_core::storage::InMemoryStorageEngine;

    let entries = 500;
    let lookups = 5_000;

    println!("\n{}", "=".repeat(70));
    println!("  L2 Cache: InMemory StorageEngine ({} entries, {} lookups)", entries, lookups);
    println!("{}\n", "=".repeat(70));

    // L1-only
    let l1_cache = TieredDataViewCache::new(DataViewCachingPolicy {
        ttl_seconds: 300, l1_enabled: true, l1_max_bytes: usize::MAX,
        l1_max_entries: entries, l2_enabled: false, l2_max_value_bytes: 131_072,
    });

    // L1 + L2
    let storage = Arc::new(InMemoryStorageEngine::new());
    let l1l2_cache = TieredDataViewCache::new(DataViewCachingPolicy {
        ttl_seconds: 300, l1_enabled: true, l1_max_bytes: usize::MAX,
        l1_max_entries: entries, l2_enabled: true, l2_max_value_bytes: 131_072,
    }).with_storage(storage);

    // Populate
    for i in 0..entries as i64 {
        let p = make_params(i);
        l1_cache.set("contacts", &p, &sample_result(i), None).await.unwrap();
        l1l2_cache.set("contacts", &p, &sample_result(i), None).await.unwrap();
    }

    // L1 write overhead
    let start = Instant::now();
    for i in 0..lookups as i64 {
        let p = make_params(i % entries as i64);
        l1_cache.set("contacts", &p, &sample_result(i), None).await.unwrap();
    }
    let l1_write = start.elapsed();

    // L1+L2 write overhead (includes serialization)
    let start = Instant::now();
    for i in 0..lookups as i64 {
        let p = make_params(i % entries as i64);
        l1l2_cache.set("contacts", &p, &sample_result(i), None).await.unwrap();
    }
    let l1l2_write = start.elapsed();

    // L1-only read
    let p0 = make_params(0);
    let start = Instant::now();
    for _ in 0..lookups {
        let _ = l1_cache.get("contacts", &p0).await;
    }
    let l1_read = start.elapsed();

    // L1+L2 read (hits L1, L2 not touched)
    let start = Instant::now();
    for _ in 0..lookups {
        let _ = l1l2_cache.get("contacts", &p0).await;
    }
    let l1l2_read = start.elapsed();

    println!("  Reads ({} lookups):", lookups);
    println!("    L1 only:   {:>10.2?}  ({:.0} ops/s)", l1_read, lookups as f64 / l1_read.as_secs_f64());
    println!("    L1+L2:     {:>10.2?}  ({:.0} ops/s)", l1l2_read, lookups as f64 / l1l2_read.as_secs_f64());
    println!();
    println!("  Writes ({} ops):", lookups);
    println!("    L1 only:   {:>10.2?}  ({:.0} ops/s)", l1_write, lookups as f64 / l1_write.as_secs_f64());
    println!("    L1+L2:     {:>10.2?}  ({:.0} ops/s)", l1l2_write, lookups as f64 / l1l2_write.as_secs_f64());
    println!("    L2 overhead: {:.1}x", l1l2_write.as_nanos() as f64 / l1_write.as_nanos() as f64);
}

// ── 5. PostgreSQL: uncached vs L1 cached ─────────────────────────

#[tokio::test]
async fn bench_5_postgres_cached_vs_uncached() {
    use rivers_core::DriverFactory;
    use rivers_core::drivers::PostgresDriver;
    use rivers_driver_sdk::{ConnectionParams, DatabaseDriver};
    use rivers_runtime::dataview::{DataViewConfig, DataViewCachingConfig};
    use rivers_runtime::dataview_engine::DataViewRegistry;
    use rivers_runtime::tiered_cache::NoopDataViewCache;

    println!("\n{}", "=".repeat(70));
    println!("  PostgreSQL: Uncached vs L1 Cached");
    println!("{}\n", "=".repeat(70));

    // Load credentials from lockbox meta sidecar
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent().and_then(|p| p.parent());
    let meta_path = match root {
        Some(r) => r.join("sec/lockbox/entries/postgres/test.meta.json"),
        None => { println!("  SKIP: Cannot find project root"); return; }
    };
    if !meta_path.exists() {
        println!("  SKIP: No LockBox credentials for postgres/test");
        return;
    }
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&meta_path).unwrap()
    ).unwrap();
    // Meta uses "hosts" array like ["192.168.2.209:5432", ...]
    let host_str = meta["hosts"].as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("localhost:5432");
    let (host, port) = if let Some(idx) = host_str.rfind(':') {
        (host_str[..idx].to_string(), host_str[idx+1..].parse().unwrap_or(5432))
    } else {
        (host_str.to_string(), 5432u16)
    };
    // Load password from .age file
    let age_path = root.unwrap().join("sec/lockbox/entries/postgres/test.age");
    let password = if age_path.exists() {
        let output = std::process::Command::new("age")
            .args(["-d", "-i"])
            .arg(root.unwrap().join("sec/lockbox/identity.key"))
            .arg(&age_path)
            .output();
        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => String::new(),
        }
    } else {
        String::new()
    };
    let pg_params = ConnectionParams {
        host: host.clone(), port,
        database: meta["database"].as_str().unwrap_or("rivers_test").to_string(),
        username: meta["username"].as_str().unwrap_or("rivers").to_string(),
        password,
        options: HashMap::new(),
    };

    // Try to connect
    let driver = PostgresDriver;
    match tokio::time::timeout(Duration::from_secs(5), driver.connect(&pg_params)).await {
        Ok(Ok(_)) => {}
        _ => { println!("  SKIP: PostgreSQL unreachable at {}:{}", host, port); return; }
    }

    let iterations = 1_000;

    let mut factory = DriverFactory::new();
    factory.register_database_driver(Arc::new(PostgresDriver));
    let factory = Arc::new(factory);

    let mut opts = HashMap::new();
    opts.insert("driver".to_string(), "postgres".to_string());
    let mut ds_params = HashMap::new();
    ds_params.insert("pg-ds".to_string(), ConnectionParams {
        host: pg_params.host.clone(), port: pg_params.port,
        database: pg_params.database.clone(), username: pg_params.username.clone(),
        password: pg_params.password.clone(), options: opts,
    });
    let ds_params = Arc::new(ds_params);

    let make_config = |caching: Option<DataViewCachingConfig>| DataViewConfig {
        name: "pg_select".into(), datasource: "pg-ds".into(),
        query: Some("SELECT generate_series(1,10) as id, 'test' as name".into()), caching,
        parameters: vec![], return_schema: None, invalidates: vec![],
        validate_result: false, strict_parameters: false,
        get_query: None, post_query: None, put_query: None, delete_query: None,
        get_schema: None, post_schema: None, put_schema: None, delete_schema: None,
        get_parameters: vec![], post_parameters: vec![],
        put_parameters: vec![], delete_parameters: vec![], streaming: false,
    };

    // Uncached
    let mut registry = DataViewRegistry::new();
    registry.register(make_config(None));
    let executor = rivers_runtime::DataViewExecutor::new(
        registry, factory.clone(), ds_params.clone(), Arc::new(NoopDataViewCache),
    );
    let start = Instant::now();
    for i in 0..iterations {
        let _ = executor.execute("pg_select", HashMap::new(), "GET", &format!("t-{}", i)).await;
    }
    let uncached = start.elapsed();

    // L1 cached
    let mut registry = DataViewRegistry::new();
    registry.register(make_config(Some(DataViewCachingConfig {
        ttl_seconds: 300, l1_enabled: true, l1_max_bytes: usize::MAX,
        l1_max_entries: 100_000, l2_enabled: false, l2_max_value_bytes: 131_072,
    })));
    let cache = Arc::new(TieredDataViewCache::new(DataViewCachingPolicy {
        ttl_seconds: 300, ..Default::default()
    }));
    let executor = rivers_runtime::DataViewExecutor::new(
        registry, factory.clone(), ds_params.clone(), cache,
    );
    let _ = executor.execute("pg_select", HashMap::new(), "GET", "warm").await;
    let start = Instant::now();
    for i in 0..iterations {
        let _ = executor.execute("pg_select", HashMap::new(), "GET", &format!("t-{}", i)).await;
    }
    let cached = start.elapsed();

    println!("  Uncached (PG every time):     {:>10.2?}  ({:.0} ops/s)",
        uncached, iterations as f64 / uncached.as_secs_f64());
    println!("  L1 Cached (hit path):         {:>10.2?}  ({:.0} ops/s)",
        cached, iterations as f64 / cached.as_secs_f64());
    println!("  Speedup:                      {:>10.1}x",
        uncached.as_nanos() as f64 / cached.as_nanos() as f64);
}
