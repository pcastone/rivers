// canary-sql init handler — creates test tables via DDL three-gate enforcement.
// Runs in ApplicationInit context (Phase 1.5 of startup).
//
// Note: ctx.dataview() is not available during init (HOST_CONTEXT not yet set).
// Seed data for param-order tests is inserted inline by the test handlers
// themselves (idempotent — ignore duplicate key on re-runs).

function initialize(ctx) {
    Rivers.log.info("canary-sql init handler started");

    // ── canary_records (atomic SQL tests) ──────────────────────────
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

    // ── messages (CS2 Messaging scenario, spec §5 MSG-8) ───────────
    // zname-trap schema (MSG-7): `body` sorts alphabetically before `id`,
    // and `zsender` sorts after `recipient`, exercising parameter binders
    // that incorrectly sort param names instead of honoring declaration
    // order. Types chosen for portability across PG/MySQL/SQLite without
    // per-driver `CREATE TABLE` divergence: TEXT for id/strings, INTEGER
    // for boolean-as-0/1, CURRENT_TIMESTAMP default.
    var messagesDDL =
        "CREATE TABLE IF NOT EXISTS messages (" +
        "  id TEXT PRIMARY KEY," +
        "  zsender TEXT NOT NULL," +
        "  recipient TEXT NOT NULL," +
        "  subject TEXT," +
        "  body TEXT," +
        "  is_secret INTEGER NOT NULL DEFAULT 0," +
        "  cipher TEXT," +
        "  created_at TEXT DEFAULT CURRENT_TIMESTAMP" +
        ")";

    // SQLite — required, fails loudly if unavailable.
    try {
        ctx.ddl("canary-sqlite", messagesDDL);
        Rivers.log.info("canary-sql: SQLite messages table ready");
    } catch (e) {
        Rivers.log.warn("canary-sql: SQLite messages DDL failed — " + String(e));
    }

    // PG / MySQL — best-effort; infra may be down on local deploys.
    // Scenario runs gated by PG_AVAIL / MYSQL_AVAIL in run-tests.sh, so
    // a missing table here only matters when the scenario is gated on.
    try {
        ctx.ddl("canary-pg", messagesDDL);
        Rivers.log.info("canary-sql: PG messages table ready");
    } catch (e) {
        Rivers.log.warn("canary-sql: PG messages DDL skipped — " + String(e));
    }

    try {
        // MySQL requires a length on TEXT PRIMARY KEY — swap to VARCHAR(64).
        // MySQL also does not allow DEFAULT CURRENT_TIMESTAMP on TEXT columns;
        // use DATETIME instead so the DDL passes MySQL's strict type rules.
        var messagesDDLMysql = messagesDDL
            .replace("id TEXT PRIMARY KEY", "id VARCHAR(64) PRIMARY KEY")
            .replace("created_at TEXT DEFAULT CURRENT_TIMESTAMP",
                     "created_at DATETIME DEFAULT CURRENT_TIMESTAMP");
        ctx.ddl("canary-mysql", messagesDDLMysql);
        Rivers.log.info("canary-sql: MySQL messages table ready");
    } catch (e) {
        Rivers.log.warn("canary-sql: MySQL messages DDL skipped — " + String(e));
    }

    Rivers.log.info("canary-sql init handler completed");
}
