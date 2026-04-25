// canary-streams init handler — creates the CS3 events table.
// DDL three-gate enforcement; runs during ApplicationInit phase.

function initialize(ctx) {
    Rivers.log.info("canary-streams init handler started");

    // CS3 / AF-9 — Activity Feed events table.
    try {
        ctx.ddl("events_db",
            "CREATE TABLE IF NOT EXISTS events (" +
            "  id TEXT PRIMARY KEY," +
            "  actor TEXT NOT NULL," +
            "  target_user TEXT NOT NULL," +
            "  event_type TEXT NOT NULL," +
            "  payload TEXT," +
            "  published_at TEXT NOT NULL," +
            "  consumed_at TEXT" +
            ")"
        );
        Rivers.log.info("canary-streams: events table ready");
    } catch (e) {
        Rivers.log.warn("canary-streams: events DDL skipped — " + String(e));
    }

    Rivers.log.info("canary-streams init handler completed");
}
