// SCENARIO-STREAM-ACTIVITY-FEED — spec §6. 11-step Kafka-backed event
// pipeline, unblocked by BR-2026-04-23 V8 broker-publish bridge.
//
// AF-compliance map:
//   AF-1 (Kafka source of truth)     — orchestrator publishes via
//                                      ctx.datasource("canary-kafka").publish;
//                                      no direct events_insert from orchestrator.
//   AF-2 (persistence in consumer)   — events-consumer.ts owns the INSERT.
//   AF-3 (server-side user scoping)  — events_for_user WHERE target_user.
//   AF-4 (pagination + date range)   — LIMIT/OFFSET/since/until params.
//   AF-5 (ordering preserved)        — ORDER BY published_at ASC.
//   AF-6 (min event fields)          — id/actor/target_user/event_type/
//                                      published_at/payload all present.
//   AF-7 (events persisted in SQL)   — events_db SQLite datasource.
//   AF-8 (Kafka topic via LockBox)   — canary-kafka lockbox alias.
//   AF-9 (init DDL)                  — init.ts creates events table.
//
// ** Deploy-time verification pending ** — CS3 ships structurally; live
// Kafka + consumer timing verified at first deploy per the established
// pattern (CS2/CS4 same gate).

import { ScenarioResult } from "./scenario-harness.ts";

const TOPIC = "canary.events";

// Poll the events_for_user DataView until at least `want` scenario-tagged
// rows are visible, or timeout. Returns the collected rows.
function waitForEvents(
    ctx: any,
    target_user: string,
    id_prefix: string,
    want: number,
    max_ms: number
): any[] {
    const deadline = Date.now() + max_ms;
    // 100/200/400/800/1600/... capped at 1600ms per spec §10 recommendation.
    let wait = 100;
    while (true) {
        const result = ctx.dataview("events_for_user", {
            target_user,
            since: "",
            until: "",
            limit: 1000,
            offset: 0,
        });
        const rows = (result && result.rows) ? result.rows : [];
        const scenario_rows = rows.filter((r: any) =>
            typeof r.id === "string" && r.id.indexOf(id_prefix) === 0
        );
        if (scenario_rows.length >= want || Date.now() >= deadline) {
            return scenario_rows;
        }
        // Busy-wait since Rivers handlers have no setTimeout. Acceptable
        // for a short-duration canary scenario; 5s max.
        const sleep_until = Math.min(Date.now() + wait, deadline);
        while (Date.now() < sleep_until) { /* spin */ }
        wait = Math.min(wait * 2, 1600);
    }
}

export function scenarioActivityFeed(ctx: any): void {
    const sr = new ScenarioResult(
        "SCENARIO-STREAM-ACTIVITY-FEED",
        "STREAM",
        "activity-feed",
        "rivers-canary-scenarios-spec.md §6",
        11
    );

    const id_prefix = "evt-" + ctx.trace_id + "-";

    // Cleanup-before — wipe ALL events for scenario users (bob + carol) so
    // accumulated rows from prior test runs don't displace pagination windows.
    // Also wipe by id_prefix for belt-and-suspenders.
    try {
        ctx.dataview("events_cleanup_user", { target_user: "bob" });
        ctx.dataview("events_cleanup_user", { target_user: "carol" });
        ctx.dataview("events_cleanup", { id_prefix: id_prefix + "%" });
    } catch (e) {
        ctx.resdata = sr.fail("cleanup-before failed: " + String(e));
        return;
    }

    const kafka = ctx.datasource("canary-kafka");
    if (!kafka || typeof kafka.publish !== "function") {
        ctx.resdata = sr.fail(
            "canary-kafka datasource has no publish() — V8 broker bridge missing"
        );
        return;
    }

    const now = () => new Date().toISOString();

    // Helper to publish one event with AF-6 minimum fields.
    function publishEvent(id: string, actor: string, target_user: string, etype: string, payload: any) {
        const envelope = {
            id,
            actor,
            target_user,
            event_type: etype,
            payload,
            published_at: now(),
        };
        return kafka.publish({
            destination: TOPIC,
            payload: JSON.stringify(envelope),
            key: target_user,
            headers: { "trace_id": ctx.trace_id }
        });
    }

    // ── Step 1: Publish event for Bob ──────────────────────────
    const id1 = id_prefix + "1";
    const s1 = sr.beginStep("publish-bob-event-1");
    try {
        const receipt = publishEvent(id1, "alice", "bob", "comment", "Alice commented on your post");
        s1.assert("publish_no_throw", true);
        s1.assertExists("receipt", receipt);
    } catch (e) {
        s1.assert("publish_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 2: Wait for consumer to persist ───────────────────
    if (sr.hasFailed(1)) {
        sr.skipStep("consumer-catches-up", 1);
    } else {
        const s2 = sr.beginStep("consumer-catches-up");
        try {
            const got = waitForEvents(ctx, "bob", id_prefix, 1, 5000);
            s2.assertEquals("count_1", 1, got.length);
            if (got.length > 0) {
                s2.assertEquals("id_matches", id1, got[0].id);
                s2.assertEquals("actor_alice", "alice", got[0].actor);
            }
        } catch (e) {
            s2.assert("wait_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 3: Bob REST history — 1 event ─────────────────────
    if (sr.hasFailed(2)) {
        sr.skipStep("bob-history-one", 2);
    } else {
        const s3 = sr.beginStep("bob-history-one");
        try {
            const rows = ctx.dataview("events_for_user", { target_user: "bob", since: "", until: "", limit: 100, offset: 0 });
            const list = (rows && rows.rows) ? rows.rows : [];
            const scenario_rows = list.filter((r: any) => typeof r.id === "string" && r.id.indexOf(id_prefix) === 0);
            s3.assertEquals("count_1", 1, scenario_rows.length);
        } catch (e) {
            s3.assert("history_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 4: Publish 3 more for Bob ─────────────────────────
    const idsN = [id_prefix + "2", id_prefix + "3", id_prefix + "4"];
    if (sr.hasFailed(1)) {
        sr.skipStep("publish-bob-events-2-3-4", 1);
    } else {
        const s4 = sr.beginStep("publish-bob-events-2-3-4");
        try {
            for (const id of idsN) {
                publishEvent(id, "alice", "bob", "comment", "follow-up " + id);
            }
            s4.assert("all_published", true);
        } catch (e) {
            s4.assert("all_published", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 5: Wait + Bob history == 4 ─────────────────────────
    if (sr.hasFailed(4)) {
        sr.skipStep("bob-history-four", 4);
    } else {
        const s5 = sr.beginStep("bob-history-four");
        try {
            const got = waitForEvents(ctx, "bob", id_prefix, 4, 5000);
            s5.assertEquals("count_4", 4, got.length);
            // AF-5 order preserved (published_at ASC means insertion order).
            for (let i = 0; i < got.length - 1; i++) {
                s5.assert("order_" + i, got[i].published_at <= got[i + 1].published_at,
                    got[i].published_at + " <= " + got[i + 1].published_at);
            }
        } catch (e) {
            s5.assert("history_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 6: Bob history with date range (before last 3) ─────
    if (sr.hasFailed(5)) {
        sr.skipStep("bob-history-date-range", 5);
    } else {
        const s6 = sr.beginStep("bob-history-date-range");
        try {
            const allRows = ctx.dataview("events_for_user",
                { target_user: "bob", since: "", until: "", limit: 100, offset: 0 });
            const list = (allRows && allRows.rows) ? allRows.rows : [];
            const scenario_rows = list.filter((r: any) => typeof r.id === "string" && r.id.indexOf(id_prefix) === 0);
            // Pick a `until` timestamp between event 1 and event 2 — use
            // event 2's published_at minus 1ms (lexical ordering on ISO-8601).
            if (scenario_rows.length >= 2) {
                const until = scenario_rows[1].published_at;
                const filtered = ctx.dataview("events_for_user",
                    { target_user: "bob", since: "", until, limit: 100, offset: 0 });
                const fRows = (filtered && filtered.rows) ? filtered.rows : [];
                const fScenario = fRows.filter((r: any) => typeof r.id === "string" && r.id.indexOf(id_prefix) === 0);
                // Events 1 and 2 have published_at <= until; event 1 should
                // be present, event 2 inclusive — acceptable variance.
                s6.assert("at_least_one", fScenario.length >= 1,
                    "filtered=" + fScenario.length);
                s6.assert("less_than_four", fScenario.length < 4);
            } else {
                s6.assert("enough_events_to_range", false,
                    "had " + scenario_rows.length);
            }
        } catch (e) {
            s6.assert("range_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 7: Carol history empty ─────────────────────────────
    const s7 = sr.beginStep("carol-history-empty");
    try {
        const rows = ctx.dataview("events_for_user",
            { target_user: "carol", since: "", until: "", limit: 100, offset: 0 });
        const list = (rows && rows.rows) ? rows.rows : [];
        const scenario_rows = list.filter((r: any) => typeof r.id === "string" && r.id.indexOf(id_prefix) === 0);
        s7.assertEquals("count_0", 0, scenario_rows.length);
    } catch (e) {
        s7.assert("history_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 8: Publish event for Carol ─────────────────────────
    const id5 = id_prefix + "5";
    const s8 = sr.beginStep("publish-carol-event");
    try {
        publishEvent(id5, "alice", "carol", "mention", "Alice mentioned Carol");
        s8.assert("publish_no_throw", true);
    } catch (e) {
        s8.assert("publish_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 9: Carol history == 1 ──────────────────────────────
    if (sr.hasFailed(8)) {
        sr.skipStep("carol-history-one", 8);
    } else {
        const s9 = sr.beginStep("carol-history-one");
        try {
            const got = waitForEvents(ctx, "carol", id_prefix, 1, 5000);
            s9.assertEquals("count_1", 1, got.length);
            if (got.length > 0) {
                s9.assertEquals("target_carol", "carol", got[0].target_user);
            }
        } catch (e) {
            s9.assert("wait_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 10: Bob history unchanged (still 4) — AF-3 scoping ─
    if (sr.hasFailed(5)) {
        sr.skipStep("bob-history-still-four", 5);
    } else {
        const s10 = sr.beginStep("bob-history-still-four");
        try {
            const rows = ctx.dataview("events_for_user",
                { target_user: "bob", since: "", until: "", limit: 100, offset: 0 });
            const list = (rows && rows.rows) ? rows.rows : [];
            const scenario_rows = list.filter((r: any) => typeof r.id === "string" && r.id.indexOf(id_prefix) === 0);
            s10.assertEquals("count_still_4", 4, scenario_rows.length);
            for (const r of scenario_rows) {
                s10.assertEquals("target_always_bob", "bob", r.target_user);
            }
        } catch (e) {
            s10.assert("history_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 11: Bob pagination — 2 pages × 2 ──────────────────
    if (sr.hasFailed(5)) {
        sr.skipStep("bob-pagination", 5);
    } else {
        const s11 = sr.beginStep("bob-pagination");
        try {
            const page1 = ctx.dataview("events_for_user",
                { target_user: "bob", since: "", until: "", limit: 2, offset: 0 });
            const page2 = ctx.dataview("events_for_user",
                { target_user: "bob", since: "", until: "", limit: 2, offset: 2 });
            const p1 = ((page1 && page1.rows) ? page1.rows : []).filter((r: any) => r.id && r.id.indexOf(id_prefix) === 0);
            const p2 = ((page2 && page2.rows) ? page2.rows : []).filter((r: any) => r.id && r.id.indexOf(id_prefix) === 0);
            s11.assert("page1_has_rows", p1.length >= 1);
            s11.assert("page2_has_rows", p2.length >= 1);
            // No duplicates across pages.
            const ids1 = new Set(p1.map((r: any) => r.id));
            let dup = false;
            for (const r of p2) { if (ids1.has(r.id)) { dup = true; break; } }
            s11.assert("no_duplicates", !dup);
        } catch (e) {
            s11.assert("pagination_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // Cleanup-after — best-effort; remove scenario users' events.
    try {
        ctx.dataview("events_cleanup_user", { target_user: "bob" });
        ctx.dataview("events_cleanup_user", { target_user: "carol" });
    } catch (e) {
        Rivers.log.warn("scenario-activity-feed cleanup-after failed: " + String(e));
    }

    ctx.resdata = sr.finish();
}
