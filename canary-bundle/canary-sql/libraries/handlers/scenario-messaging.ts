// SCENARIO-SQL-MESSAGING — spec §5. 12-step user messaging workflow run
// against one of three SQL backends. Orchestrator-as-single-handler per
// revised CS0.2 (identity passed as DataView parameter; Rivers.http not
// available for pre-seeded-session dispatch).
//
// MSG-compliance map:
//   MSG-1 (sender from session)    — not directly verified (see CS0.2 note).
//   MSG-2 (server-side inbox)      — msg_inbox_* WHERE recipient = $recipient.
//   MSG-3 (search by keyword)      — msg_search_* WHERE body LIKE $pattern.
//   MSG-4 (real encryption)        — PLACEHOLDER XOR cipher; canary-sql has no
//                                    [[keystores]] config so Rivers.crypto.encrypt
//                                    isn't available. Scenario still exercises
//                                    the ciphertext-at-rest invariant (MSG-9).
//   MSG-5 (delete sender-or-recipient) — msg_delete_* WHERE id=? AND (zsender=? OR recipient=?).
//   MSG-6 (parameterised)          — all queries use $-params, no concatenation.
//   MSG-7 (zname-trap)             — column `body` sorts alphabetically before `id`;
//                                    `zsender` sorts after `recipient`. See init.ts DDL.
//   MSG-8 (init DDL)               — canary-sql/libraries/handlers/init.ts.
//   MSG-9 (ciphertext at rest)     — msg_select_cipher_* direct-probe proves raw
//                                    column ≠ plaintext.
//   MSG-10 (multi-driver)          — three entrypoints share runScenario().

import { ScenarioResult } from "./scenario-harness.ts";

// Placeholder encryption (MSG-4 deviation — see file header + CS0.2).
// XOR with a fixed key + base64. NOT secure; proves only the at-rest
// invariant (stored ≠ plaintext). Replace with Rivers.crypto.encrypt
// when canary-sql gains a [[keystores]] section.
const PLACEHOLDER_KEY = "canary-scenario-xor-key-placeholder";
// Hex encoding — V8's default scope has no btoa/atob (browser-only).
function placeholderEncrypt(plaintext: string): string {
    var out = "";
    for (var i = 0; i < plaintext.length; i++) {
        var x = plaintext.charCodeAt(i) ^ PLACEHOLDER_KEY.charCodeAt(i % PLACEHOLDER_KEY.length);
        var h = (x & 0xff).toString(16);
        if (h.length < 2) h = "0" + h;
        out += h;
    }
    return out;
}
function placeholderDecrypt(ciphertext: string): string {
    var out = "";
    for (var i = 0; i < ciphertext.length; i += 2) {
        var byte = parseInt(ciphertext.substr(i, 2), 16);
        var idx = i / 2;
        out += String.fromCharCode(
            byte ^ PLACEHOLDER_KEY.charCodeAt(idx % PLACEHOLDER_KEY.length)
        );
    }
    return out;
}

// DataView name resolver — each DataView has a driver-suffixed variant.
function dv(driver: string, base: string): string {
    return base + "_" + driver;
}

// Extract rows from a ctx.dataview() result which may be {rows: [...]} or
// a bare array depending on driver/DataView kind.
function rowsOf(result: any): any[] {
    if (result && Array.isArray(result.rows)) return result.rows;
    if (Array.isArray(result)) return result;
    return [];
}

function runMessagingScenario(ctx: any, driver: string): any {
    var test_id = "SCENARIO-SQL-MESSAGING-" + driver.toUpperCase();
    var sr = new ScenarioResult(
        test_id,
        "SQL",
        "messaging",
        "rivers-canary-scenarios-spec.md §5",
        12
    );

    // Unique per-run prefix — cleanup-before + cleanup-after use LIKE
    // against this to stay isolated from prior runs / other scenarios.
    var id_prefix = "msg-" + ctx.trace_id + "-";

    // Cleanup-before: prior runs shouldn't contaminate this one's
    // assertions about inbox counts.
    try {
        ctx.dataview(dv(driver, "msg_cleanup"), { id_prefix: id_prefix + "%" });
    } catch (e) {
        return sr.fail("cleanup-before failed: " + String(e));
    }

    var id1 = id_prefix + "1";
    var id6 = id_prefix + "6";
    var plaintext_1 = "Hello Bob, let's grab coffee soon. Keyword: pineapple.";
    var plaintext_6 = "Confidential: meet me at 3pm. Secret-keyword: quail.";

    // ── Step 1: Alice → Bob ─────────────────────────────────────
    var s1 = sr.beginStep("alice-sends-to-bob");
    try {
        // Non-secret message: omit `cipher` (null string fails param
        // binder's type check); use empty string for consistent schema.
        ctx.dataview(dv(driver, "msg_insert"), {
            id: id1,
            zsender: "alice",
            recipient: "bob",
            subject: "Coffee?",
            body: plaintext_1,
            is_secret: 0,
            cipher: ""
        });
        s1.assert("insert_no_throw", true);
    } catch (e) {
        s1.assert("insert_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 2: Bob inbox shows Alice's message ─────────────────
    if (sr.hasFailed(1)) {
        sr.skipStep("bob-inbox-has-alice-message", 1);
    } else {
        var s2 = sr.beginStep("bob-inbox-has-alice-message");
        try {
            var rows2 = rowsOf(ctx.dataview(dv(driver, "msg_inbox"), { recipient: "bob" }));
            var found = null;
            for (var i = 0; i < rows2.length; i++) {
                if (rows2[i].id === id1) { found = rows2[i]; break; }
            }
            s2.assertExists("alice_message_found", found);
            if (found) {
                s2.assertEquals("sender_is_alice", "alice", found.zsender);
                s2.assertEquals("recipient_is_bob", "bob", found.recipient);
                var b = String(found.body || "");
                s2.assert("body_contains_keyword", b.indexOf("pineapple") !== -1,
                    "body=" + b);
            }
        } catch (e) {
            s2.assert("inbox_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 3: Alice inbox — no scenario messages ──────────────
    var s3 = sr.beginStep("alice-inbox-empty");
    try {
        var rows3 = rowsOf(ctx.dataview(dv(driver, "msg_inbox"), { recipient: "alice" }));
        var aliceHas = false;
        for (var j = 0; j < rows3.length; j++) {
            var jid = String(rows3[j].id || "");
            if (jid.indexOf(id_prefix) === 0) { aliceHas = true; break; }
        }
        s3.assert("alice_has_no_scenario_messages", !aliceHas);
    } catch (e) {
        s3.assert("alice_inbox_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 4: Bob keyword search — hit ────────────────────────
    if (sr.hasFailed(1)) {
        sr.skipStep("bob-search-hits", 1);
    } else {
        var s4 = sr.beginStep("bob-search-hits");
        try {
            var rows4 = rowsOf(ctx.dataview(dv(driver, "msg_search"),
                { recipient: "bob", pattern: "%pineapple%" }));
            // Count rows from THIS scenario run.
            var hits = 0;
            var firstHit = null;
            for (var k = 0; k < rows4.length; k++) {
                if (String(rows4[k].id || "").indexOf(id_prefix) === 0) {
                    hits++;
                    if (!firstHit) firstHit = rows4[k];
                }
            }
            s4.assertEquals("search_count_is_1", 1, hits);
            if (firstHit) {
                s4.assertEquals("hit_id_matches", id1, firstHit.id);
            }
        } catch (e) {
            s4.assert("search_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 5: Bob keyword search — miss ───────────────────────
    var s5 = sr.beginStep("bob-search-misses");
    try {
        var rows5 = rowsOf(ctx.dataview(dv(driver, "msg_search"),
            { recipient: "bob", pattern: "%jellyfish-xylophone-unlikely%" }));
        s5.assertEquals("search_empty", 0, rows5.length);
    } catch (e) {
        s5.assert("search_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 6: Alice sends secret to Bob ───────────────────────
    var cipher6 = placeholderEncrypt(plaintext_6);
    var s6 = sr.beginStep("alice-sends-secret");
    try {
        // Secret message: body empty (ciphertext carries content).
        ctx.dataview(dv(driver, "msg_insert"), {
            id: id6,
            zsender: "alice",
            recipient: "bob",
            subject: "[secret]",
            body: "",
            is_secret: 1,
            cipher: cipher6
        });
        s6.assert("secret_insert_no_throw", true);
        s6.assert("cipher_differs_from_plaintext", cipher6 !== plaintext_6);
    } catch (e) {
        s6.assert("secret_insert_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 7: Bob decrypts the secret ─────────────────────────
    if (sr.hasFailed(6)) {
        sr.skipStep("bob-decrypts-secret", 6);
    } else {
        var s7 = sr.beginStep("bob-decrypts-secret");
        try {
            var rows7 = rowsOf(ctx.dataview(dv(driver, "msg_inbox"), { recipient: "bob" }));
            var secret = null;
            for (var l = 0; l < rows7.length; l++) {
                if (rows7[l].id === id6) { secret = rows7[l]; break; }
            }
            s7.assertExists("secret_row_found", secret);
            if (secret && secret.cipher) {
                var decrypted = placeholderDecrypt(String(secret.cipher));
                s7.assertEquals("decrypted_matches_plaintext", plaintext_6, decrypted);
            } else {
                s7.assert("secret_row_has_cipher", false,
                    "cipher col was " + (secret ? typeof secret.cipher : "no-row"));
            }
        } catch (e) {
            s7.assert("decrypt_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 8: Direct SELECT proves ciphertext at rest (MSG-9) ─
    if (sr.hasFailed(6)) {
        sr.skipStep("ciphertext-at-rest", 6);
    } else {
        var s8 = sr.beginStep("ciphertext-at-rest");
        try {
            var rows8 = rowsOf(ctx.dataview(dv(driver, "msg_select_cipher"), { id: id6 }));
            s8.assertEquals("one_row", 1, rows8.length);
            if (rows8.length > 0) {
                var raw_cipher = String(rows8[0].cipher || "");
                s8.assert("cipher_not_plaintext", raw_cipher !== plaintext_6,
                    "stored=" + raw_cipher.substring(0, 20) + "...");
                s8.assertEquals("cipher_matches_stored", cipher6, raw_cipher);
            }
        } catch (e) {
            s8.assert("at_rest_probe_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 9: Carol inbox is scoped (no scenario messages) ────
    var s9 = sr.beginStep("carol-inbox-scoped");
    try {
        var rows9 = rowsOf(ctx.dataview(dv(driver, "msg_inbox"), { recipient: "carol" }));
        var carolHas = false;
        for (var m = 0; m < rows9.length; m++) {
            if (String(rows9[m].id || "").indexOf(id_prefix) === 0) {
                carolHas = true; break;
            }
        }
        s9.assert("carol_has_no_scenario_messages", !carolHas);
    } catch (e) {
        s9.assert("carol_inbox_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 10: Bob deletes Alice's non-secret message ─────────
    if (sr.hasFailed(1)) {
        sr.skipStep("bob-deletes-non-secret", 1);
    } else {
        var s10 = sr.beginStep("bob-deletes-non-secret");
        try {
            var r10 = ctx.dataview(dv(driver, "msg_delete"), { id: id1, actor: "bob" });
            s10.assert("delete_no_throw", true);
            if (r10 && typeof r10.affected_rows === "number") {
                s10.assertEquals("affected_rows_1", 1, r10.affected_rows);
            }
        } catch (e) {
            s10.assert("delete_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 11: Bob inbox — only the secret remains ────────────
    if (sr.hasFailed(6)) {
        sr.skipStep("bob-inbox-only-secret", 6);
    } else {
        var s11 = sr.beginStep("bob-inbox-only-secret");
        try {
            var rows11 = rowsOf(ctx.dataview(dv(driver, "msg_inbox"), { recipient: "bob" }));
            var scenario_rows: any[] = [];
            for (var n = 0; n < rows11.length; n++) {
                if (String(rows11[n].id || "").indexOf(id_prefix) === 0) {
                    scenario_rows.push(rows11[n]);
                }
            }
            s11.assertEquals("scenario_count_is_1", 1, scenario_rows.length);
            if (scenario_rows.length > 0) {
                s11.assertEquals("remaining_is_secret", id6, scenario_rows[0].id);
            }
        } catch (e) {
            s11.assert("inbox_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 12: Carol cannot delete Bob's secret (MSG-5) ───────
    if (sr.hasFailed(6)) {
        sr.skipStep("carol-cannot-delete-secret", 6);
    } else {
        var s12 = sr.beginStep("carol-cannot-delete-secret");
        try {
            var r12 = ctx.dataview(dv(driver, "msg_delete"), { id: id6, actor: "carol" });
            s12.assert("delete_call_no_throw", true);
            if (r12 && typeof r12.affected_rows === "number") {
                s12.assertEquals("carol_delete_blocked", 0, r12.affected_rows);
            }
            // Verify secret still exists.
            var check = rowsOf(ctx.dataview(dv(driver, "msg_select_cipher"), { id: id6 }));
            s12.assertEquals("secret_still_present", 1, check.length);
        } catch (e) {
            s12.assert("delete_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // Cleanup-after — best-effort; §10 says cleanup failure shouldn't
    // flip the verdict.
    try {
        ctx.dataview(dv(driver, "msg_cleanup"), { id_prefix: id_prefix + "%" });
    } catch (e) {
        Rivers.log.warn("scenario-messaging cleanup-after failed: " + String(e));
    }

    return sr.finish();
}

export function scenarioMessagingPg(ctx: any): void {
    ctx.resdata = runMessagingScenario(ctx, "pg");
}

export function scenarioMessagingMysql(ctx: any): void {
    ctx.resdata = runMessagingScenario(ctx, "mysql");
}

export function scenarioMessagingSqlite(ctx: any): void {
    ctx.resdata = runMessagingScenario(ctx, "sqlite");
}
