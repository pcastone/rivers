// Codecomponent handlers for the cb-rivers feature validation bundle.
// One entrypoint per case. Each handler returns the smallest result that
// lets run-probe.sh decide PASS/FAIL.
//
// MIGRATION NOTES (v0.60.12 → CB probe alignment, 2026-05-09):
//
//   - caseH now reads args.path_params (P1.9 canonical surface) before
//     falling back to ctx.request.path_params (REST sanity check).
//     Per rivers-mcp-view-spec.md §10.4: MCP `view = "..."` dispatch
//     puts matched URL path-segment values on args.path_params (top-level),
//     mirroring the MCP view's own MatchedRoute, NOT the inner referent.
//
//   - caseFGuard added (P1.10): named-guard recipe target. Returns
//     { allow: bool } based on the X-Case-F-Allow header.
//
//   - caseGBearerGuard added (P1.12 closed-as-superseded → §11.5 recipe):
//     Authorization: Bearer <token> validation. Returns { allow: bool }.
//
//   - caseICronTick added (P1.14, future): writes a sentinel row each
//     tick — probe polls for it. Stays inert until Rivers ships
//     view_type = "Cron" (Sprint 2026-05-09 Track 3).

type Ctx = {
    request: {
        headers?: Record<string, string>;
        body?: unknown;
        path_params?: Record<string, string>;
    };
    session?: unknown;
    resdata: unknown;
};

// MCP `view = "..."` dispatch passes a top-level args object to the
// handler. The shape is { request, session, path_params } per
// rivers-mcp-view-spec.md §10.4. REST dispatch does NOT pass `args` —
// path_params lives on ctx.request.path_params there.
declare const args: {
    request?: unknown;
    session?: unknown;
    path_params?: Record<string, string>;
} | undefined;

declare const Rivers: {
    db: {
        query(name: string, sql: string, params: unknown[]): Promise<{ rows: any[] }>;
        execute(name: string, sql: string, params: unknown[]): Promise<unknown>;
    };
};

function ok(ctx: Ctx, body: unknown): void {
    ctx.resdata = { status: 200, body };
}

// ─── Case B — view-dispatch sentinel ────────────────────────────────
// MCP `view = "..."` should route here. Returns a hardcoded sentinel.

export async function caseB(ctx: Ctx): Promise<void> {
    ok(ctx, { case: "B", marker: "view-dispatch-OK" });
}

// ─── Case C — capability propagation through MCP `view = "..."` ─────
// If P1.13 regresses, Rivers.db.query throws CapabilityError here.
// On v0.60.12 with the fix, this returns the row count.

export async function caseC(ctx: Ctx): Promise<void> {
    const r = await Rivers.db.query("probe_db", "SELECT COUNT(*) AS n FROM probe_rows", []);
    ok(ctx, { case: "C", marker: "db-query-OK", n: r.rows[0]?.n ?? null });
}

// ─── Case D — bearer reaches codecomponent via ctx.session ──────────
// MCP dispatch puts the auth_context on ctx.session as
// {kind: "bearer", token: "..."}. Probe sends Authorization: Bearer <X>;
// handler echoes ctx.session.kind plus a redacted token-length so we
// don't leak the value into stdout.

export async function caseD(ctx: Ctx): Promise<void> {
    const s = (ctx.session ?? {}) as { kind?: string; token?: string };
    const tokLen = typeof s.token === "string" ? s.token.length : 0;
    ok(ctx, {
        case: "D",
        session_kind: s.kind ?? null,
        token_length: tokLen,
        marker: s.kind === "bearer" && tokLen > 0 ? "bearer-OK" : "bearer-MISSING",
    });
}

// ─── Case F — named-guard target (P1.10) ────────────────────────────
// guard_view target. Returns { allow: true } iff X-Case-F-Allow == "yes".
// Probe sends/omits the header to exercise both branches.

export async function caseFGuard(ctx: Ctx): Promise<void> {
    const flag = ctx.request.headers?.["x-case-f-allow"] ?? "";
    ok(ctx, { allow: flag === "yes" });
}

// ─── Case G — bearer guard recipe (§11.5) ───────────────────────────
// Probe-only acceptable token. Real CB code hashes against api_keys.

export async function caseGBearerGuard(ctx: Ctx): Promise<void> {
    const auth = (ctx.request.headers?.["authorization"] ?? "").trim();
    const prefix = "Bearer ";
    if (!auth.startsWith(prefix)) {
        ok(ctx, { allow: false });
        return;
    }
    const token = auth.slice(prefix.length).trim();
    ok(ctx, { allow: token === "test-bearer-value-12345" });
}

// ─── Case H — path_params via MCP dispatch (P1.9) ───────────────────
// Reads BOTH surfaces and reports which populated:
//   - args.path_params  → MCP dispatch (P1.9 canonical)
//   - ctx.request.path_params → REST dispatch (sanity check)
// 'source' field tells the probe which path produced the values.

export async function caseH(ctx: Ctx): Promise<void> {
    const fromArgs = (typeof args !== "undefined" && args && args.path_params) || {};
    const fromCtx  = ctx.request.path_params ?? {};
    const pp = Object.keys(fromArgs).length > 0 ? fromArgs : fromCtx;
    const source = Object.keys(fromArgs).length > 0
        ? "args"
        : Object.keys(fromCtx).length > 0
            ? "ctx"
            : "missing";
    ok(ctx, {
        case: "H",
        source,
        path_params: pp,
        marker: Object.keys(pp).length > 0 ? "path-params-OK" : "path-params-MISSING",
    });
}

// ─── Case I — cron tick (P1.14 — pending Track 3) ───────────────────
// Sentinel write so the probe can poll for tick evidence.
// Inert today (Cron view_type doesn't exist yet); reachable once Track 3 ships.

export async function caseICronTick(_ctx: Ctx): Promise<void> {
    await Rivers.db.execute(
        "probe_db",
        "INSERT INTO cron_ticks(ts) VALUES (strftime('%s','now'))",
        []
    );
}
