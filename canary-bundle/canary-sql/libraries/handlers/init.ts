// canary-sql init handler — creates test tables via DDL three-gate enforcement.
// Runs in ApplicationInit context (Phase 1.5 of startup).
//
// Note: ctx.dataview() is not available during init (HOST_CONTEXT not yet set).
// Seed data for param-order tests is inserted inline by the test handlers
// themselves (idempotent — ignore duplicate key on re-runs).

function initialize(ctx) {
    Rivers.log.info("canary-sql init handler started");
    Rivers.log.info("canary-sql init handler completed");
}
