//! End-to-end LockBox integration test.
//!
//! Creates a keystore, stores a Redis credential, resolves it via LockBox,
//! and uses the resolved password to connect to Redis and execute a ping.
//! Set RIVERS_TEST_REDIS_HOST (default: localhost). If Redis is unreachable the test SKIPs and passes.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use rivers_core::drivers::RedisDriver;
use rivers_core::lockbox::{
    decrypt_keystore, encrypt_keystore, fetch_secret_value, Keystore, KeystoreEntry,
    LockBoxResolver,
};
use rivers_driver_sdk::{ConnectionParams, DatabaseDriver, Query};

fn redis_host() -> String {
    std::env::var("RIVERS_TEST_REDIS_HOST").unwrap_or_else(|_| "localhost".to_string())
}

const REDIS_PORT: u16 = 6379;
const TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn lockbox_credential_resolves_and_connects_redis() {
    // 1. Generate identity
    use age::secrecy::ExposeSecret;
    let identity = age::x25519::Identity::generate();
    let recipient = identity.to_public();
    let now = chrono::Utc::now();

    // 2. Create keystore with redis credential
    let entries = vec![KeystoreEntry {
        name: "redis/test".to_string(),
        value: "rivers_test".to_string(),
        entry_type: "string".to_string(),
        aliases: vec!["cache-test".to_string()],
        created: now,
        updated: now,
    }];

    let keystore = Keystore {
        version: 1,
        entries,
    };

    // 3. Encrypt to temp file
    let dir = tempfile::TempDir::new().unwrap();
    let keystore_path = dir.path().join("e2e.rkeystore");
    encrypt_keystore(&keystore_path, &recipient.to_string(), &keystore).unwrap();

    // Verify file was created with correct permissions
    assert!(keystore_path.exists(), "keystore file should exist");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let mode = std::fs::metadata(&keystore_path).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o600, "keystore should have 0600 permissions");
    }

    // 4. Decrypt and verify round-trip
    let identity_str = identity.to_string();
    let decrypted =
        decrypt_keystore(&keystore_path, identity_str.expose_secret().trim()).unwrap();
    assert_eq!(decrypted.entries.len(), 1);
    assert_eq!(decrypted.entries[0].name, "redis/test");
    assert_eq!(decrypted.entries[0].value, "rivers_test");

    // 5. Build resolver and resolve by name
    let resolver = LockBoxResolver::from_entries(&decrypted.entries).unwrap();
    let metadata = resolver.resolve("redis/test").unwrap();
    assert_eq!(metadata.name, "redis/test");
    assert_eq!(metadata.entry_index, 0);

    // 6. Also resolve by alias
    let alias_meta = resolver.resolve("cache-test").unwrap();
    assert_eq!(alias_meta.name, "redis/test");

    // 7. Fetch secret value from disk
    let resolved = fetch_secret_value(
        metadata,
        &keystore_path,
        identity_str.expose_secret().trim(),
    )
    .unwrap();
    assert_eq!(resolved.value, "rivers_test");

    // 8. Use the resolved password to connect to Redis
    let host = redis_host();
    let params = ConnectionParams {
        host: host.clone(),
        port: REDIS_PORT,
        database: "0".into(),
        username: "".into(),
        password: resolved.value,
        options: HashMap::new(),
    };

    let driver = RedisDriver::new();
    let conn_result = tokio::time::timeout(TIMEOUT, driver.connect(&params)).await;
    let mut conn = match conn_result {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            eprintln!("SKIP: Redis unreachable at {host}:{REDIS_PORT} — {e}");
            return;
        }
        Err(_) => {
            eprintln!("SKIP: Redis timed out at {host}:{REDIS_PORT}");
            return;
        }
    };

    // 9. Ping should succeed — proving the LockBox-resolved credential works
    let ping = Query::with_operation("ping", "redis", "PING");
    match tokio::time::timeout(TIMEOUT, conn.execute(&ping)).await {
        Ok(Ok(result)) => {
            assert_eq!(result.affected_rows, 0);
            println!("LockBox E2E: Redis ping PASSED with resolved credential");
        }
        Ok(Err(e)) => {
            let msg = format!("{e}");
            if msg.contains("Moved") || msg.contains("MOVED") {
                eprintln!("SKIP: Redis cluster MOVED — credential was valid");
            } else {
                panic!("Redis ping failed with resolved credential: {e}");
            }
        }
        Err(_) => panic!("Redis ping timed out"),
    }
}

#[tokio::test]
async fn lockbox_resolver_from_common_helper_works() {
    // Verify the shared TestCredentials helper creates a valid keystore
    let creds = common::TestCredentials::new();

    // All expected entries should be resolvable
    let redis_pw = creds.get("redis/test");
    assert_eq!(redis_pw, "rivers_test");

    let pg_pw = creds.get("postgres/test");
    assert_eq!(pg_pw, "postgres");

    let mysql_pw = creds.get("mysql/test");
    assert_eq!(mysql_pw, "root");

    let rmq_pw = creds.get("rabbitmq/test");
    assert_eq!(rmq_pw, "guest");

    let couch_pw = creds.get("couchdb/test");
    assert_eq!(couch_pw, "admin");

    let influx_pw = creds.get("influxdb/test");
    assert_eq!(influx_pw, "rivers-test");

    let empty_pw = creds.get("memcached/test");
    assert_eq!(empty_pw, "");

    println!("LockBox common helper: all credentials resolved correctly");
}

#[test]
fn lockbox_missing_credential_panics() {
    let creds = common::TestCredentials::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        creds.get("nonexistent/key")
    }));
    assert!(result.is_err(), "get() should panic for missing credential");
}
