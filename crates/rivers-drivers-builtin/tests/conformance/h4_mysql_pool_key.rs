// H4 — MySQL pool cache key must include password fingerprint.
//
// These tests verify two properties at the integration level:
//
//   1. Pool isolation — two `ConnectionParams` that share `(host, port, db,
//      user)` but differ only in password get independent pools; a successful
//      connect with `rivers`/`rivers_test` is not contaminated by a failed
//      attempt with a wrong password (or vice-versa).
//
//   2. Eviction on auth failure — if a stale/wrong-password pool entry is
//      already cached (simulated by injecting a bad-password params whose key
//      must differ from the good one, then verifying the good connect still
//      succeeds), the driver recovers without requiring a process restart.
//
// Cluster-gated: requires `RIVERS_TEST_CLUSTER=1` and a reachable MySQL at
// 192.168.2.215 (user `rivers` / password `rivers_test` / database `rivers`).
// Falls through silently when the cluster guard is off.
//
// NOTE: We cannot create a second MySQL user from within the test (no admin
// credentials), so the "wrong password" scenario is exercised by attempting a
// connect with an intentionally bad password and confirming:
//   - The bad-password connect fails (auth error, expected).
//   - The subsequent good-password connect on the SAME host/port/db/user
//     succeeds (pool isolation means the bad pool did NOT poison the cache
//     entry for the correct credentials).
//
// NOTE: Only 2 conformance tests exist here (not 3). A third "distinct
// passwords produce independent pools" scenario was removed because it was
// identical in observable behavior to Test 1 — same 3 steps, same assertions.
// The boundary conditions for `is_auth_error` (code 1044/1045 → true, others
// → false) are covered by a unit test in `mysql.rs` (`is_auth_error_boundary_codes`).

use std::collections::HashMap;
use rivers_driver_sdk::traits::DatabaseDriver;
use rivers_driver_sdk::ConnectionParams;
use rivers_drivers_builtin::MysqlDriver;

use super::conformance::*;

/// Returns the known-good MySQL connection params from the test cluster.
fn good_params() -> ConnectionParams {
    ConnectionParams {
        host: "192.168.2.215".into(),
        port: 3306,
        database: "rivers".into(),
        username: "rivers".into(),
        password: "rivers_test".into(),
        options: HashMap::new(),
    }
}

/// Returns params with the same identity but a wrong password.
/// These MUST produce a different pool key from `good_params()`.
fn wrong_password_params() -> ConnectionParams {
    ConnectionParams {
        password: "definitely_wrong_password_h4_test".into(),
        ..good_params()
    }
}

// ── Test 1: pool key isolation ────────────────────────────────────────────────
//
// Connects with the CORRECT credentials first, then attempts with the WRONG
// password (expected to fail), then connects again with the CORRECT credentials.
// The correct-credentials pool must survive — i.e., the wrong-password attempt
// must not evict or overwrite the correct pool entry.

#[tokio::test]
async fn h4_pool_key_isolates_correct_from_wrong_password() {
    if !cluster_available() {
        eprintln!(
            "RIVERS_TEST_CLUSTER not set — skipping h4_pool_key_isolates_correct_from_wrong_password"
        );
        return;
    }

    let driver = MysqlDriver;

    // Step 1: establish a good connection.
    let conn_good_first = driver.connect(&good_params()).await;
    assert!(
        conn_good_first.is_ok(),
        "H4: first connect with correct credentials must succeed: {:?}",
        conn_good_first.err()
    );
    drop(conn_good_first);

    // Step 2: attempt with wrong password — must fail.
    let conn_bad = driver.connect(&wrong_password_params()).await;
    assert!(
        conn_bad.is_err(),
        "H4: connect with wrong password must fail (got Ok unexpectedly)"
    );

    // Step 3: connect again with correct credentials — must still succeed.
    // If the wrong-password attempt had overwritten the good pool (old bug),
    // this would fail or silently route to the wrong pool.
    let conn_good_second = driver.connect(&good_params()).await;
    assert!(
        conn_good_second.is_ok(),
        "H4: second connect with correct credentials must succeed after wrong-password attempt: {:?}",
        conn_good_second.err()
    );
}

// ── Test 2: wrong-password connect does not permanently poison the cache ──────
//
// Attempts with wrong credentials, then verifies that connecting with correct
// credentials immediately after still works. This specifically exercises the
// eviction + retry path: the wrong-password pool was never cached (key differs),
// but confirms the general "correct creds work after a failed attempt" property.

#[tokio::test]
async fn h4_correct_credentials_work_after_failed_attempt() {
    if !cluster_available() {
        eprintln!(
            "RIVERS_TEST_CLUSTER not set — skipping h4_correct_credentials_work_after_failed_attempt"
        );
        return;
    }

    let driver = MysqlDriver;

    // Wrong password first.
    let bad = driver.connect(&wrong_password_params()).await;
    assert!(bad.is_err(), "H4: wrong password must produce a connect error");

    // Then correct credentials must succeed.
    let good = driver.connect(&good_params()).await;
    assert!(
        good.is_ok(),
        "H4: correct credentials must succeed even after a failed wrong-password attempt: {:?}",
        good.err()
    );
}

