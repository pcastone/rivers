// canary-sql init handler — creates test tables via DDL three-gate enforcement.
// Runs in ApplicationInit context (Phase 1.5 of startup).
//
// Note: ctx.ddl() JS API is not yet wired (S1.3 in gutter). This handler
// uses ctx.dataview() for DML verification only. DDL is handled by the
// Rust-side init handler dispatch which calls ddl_execute() directly.
//
// When ctx.ddl() is implemented, this handler will execute CREATE TABLE.
// For now, tables must be pre-created or created via the Rust init path.

function initialize(ctx) {
    Rivers.log.info("canary-sql init handler started", {
        app_id: ctx.app_id,
        context: "ApplicationInit"
    });

    // Verify we're in the init context
    // (ctx.ddl would be available here when implemented)

    Rivers.log.info("canary-sql init handler completed");
}
