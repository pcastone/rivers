// canary-sql init handler — creates test tables via DDL three-gate enforcement.
// Runs in ApplicationInit context (Phase 1.5 of startup).
//
// Note: ctx.dataview() is not available during init (HOST_CONTEXT not yet set).
// Seed data for param-order tests is inserted inline by the test handlers
// themselves (idempotent — ignore duplicate key on re-runs).

function initialize(ctx) {
    Rivers.log.info("canary-sql init handler started");

    // DDL: Create the canary_records table for SQLite tests.
    // This proves DDL goes to disk (bugreport_2026-04-06.md).
    // Uses ddl_execute which is only available in ApplicationInit context.
    try {
        ctx.ddl("canary-sqlite",
            "CREATE TABLE IF NOT EXISTS canary_records (" +
            "  id TEXT PRIMARY KEY," +
            "  zname TEXT NOT NULL," +
            "  age INTEGER NOT NULL" +
            ")"
        );
        Rivers.log.info("canary-sql: SQLite canary_records table created (or already exists)");
    } catch (e) {
        Rivers.log.warn("canary-sql: SQLite DDL skipped — " + String(e));
    }

    Rivers.log.info("canary-sql init handler completed");
}
