// SCENARIO-RUNTIME-DOC-PIPELINE — spec §7. Document workspace scenario
// hosted in canary-handlers per CS0.1. 14 steps — all live as of CS4.9.
//
// DOC-compliance map:
//   DOC-1 (no shell file ops)       — all file ops go through ctx.datasource("fs_workspace").
//   DOC-2 (chroot sandbox)          — exercised by step 11 (fs driver rejects ../ write).
//   DOC-3 (exec hash-pinned)        — step 9: unknown-command dispatch rejected by rivers-exec.
//   DOC-4 (structured exec output)  — step 8: wc-json.sh emits {lines,words,bytes} JSON.
//   DOC-5 (search returns refs)     — step 6: structured search with path-matching.
//   DOC-6 (traversal at driver)     — step 10: wc-json.sh rejects "../" at script layer;
//                                     step 11: fs driver rejects write with traversal name.
//   DOC-7 (workspace via resources) — fs_workspace declared in resources.toml + app.toml.
//   DOC-8 (cleanup best-effort)     — scenario end + any early-fail.
//   DOC-9 (privilege drop)          — rivers-exec `run_as_user` config; not directly tested.
//   DOC-10 (stat structured)        — step 14 asserts size + mtime present.

import { ScenarioResult } from "./scenario-harness.ts";

// Filesystem direct-dispatch auto-unwraps single-row results:
//   0 rows    → null
//   1 row × 1 col → the scalar (string / object)
//   N rows    → array
// grep returns 0/1/N match objects; normalise all shapes to an array.
// Single-match auto-unwrap produces an object with path/line fields at the root.
function normaliseGrepRows(hits: any): any[] {
    if (hits === null || hits === undefined) return [];
    if (Array.isArray(hits)) return hits;
    if (typeof hits === "object") {
        // filesystem driver grep returns `{results: [...], truncated: bool}`.
        if (Array.isArray(hits.results)) return hits.results;
        if (Array.isArray(hits.matches)) return hits.matches;
        if (Array.isArray(hits.entries)) return hits.entries;
        // Single unwrap — object with path/file field.
        if (typeof hits.path === "string" || typeof hits.file === "string") return [hits];
    }
    if (typeof hits === "string") return [hits];
    return [];
}

// readDir returns entry objects; normalise all shapes to a `string[]` of names.
function normaliseFsListing(listing: any): string[] {
    if (listing === null || listing === undefined) return [];
    if (typeof listing === "string") return [listing];
    if (Array.isArray(listing)) {
        return listing.map((e: any) => typeof e === "string" ? e : (e && e.name) || "");
    }
    if (typeof listing === "object") {
        if (Array.isArray(listing.entries)) {
            return listing.entries.map((e: any) => typeof e === "string" ? e : (e && e.name) || "");
        }
        // Single-entry unwrap → object with a `name` field.
        if (typeof listing.name === "string") return [listing.name];
    }
    return [];
}

export function scenarioDocPipeline(ctx: any): void {
    var sr = new ScenarioResult(
        "SCENARIO-RUNTIME-DOC-PIPELINE",
        "RUNTIME",
        "doc-pipeline",
        "rivers-canary-scenarios-spec.md §7",
        14
    );

    var fs = ctx.datasource("fs_workspace");
    // Workspace path is relative to the chroot root (/tmp).
    var work = "canary-docs-" + ctx.trace_id;

    // ── Cleanup-before: fresh workspace each run ─────────────────
    try {
        // fs driver: no recursive-delete primitive; walk + delete.
        // If the dir doesn't exist this is a no-op.
        if (fs.exists(work)) {
            var existingNames = normaliseFsListing(fs.readDir(work));
            for (var i = 0; i < existingNames.length; i++) {
                try { fs.delete(work + "/" + existingNames[i]); } catch (_e) { /* ignore */ }
            }
            try { fs.delete(work); } catch (_e) { /* ignore */ }
        }
    } catch (e) {
        return ctx.resdata = sr.fail("cleanup-before failed: " + String(e));
    }

    var report_body =
        "# Report\n\n" +
        "This document contains analysis findings.\n" +
        "Keyword: pineapple-42-unique.\n\n" +
        "Findings:\n" +
        "- One\n" +
        "- Two\n" +
        "- Three\n";
    var notes_body =
        "# Notes\n\n" +
        "Side commentary only. No distinctive keyword here.\n";

    // ── Step 1: Create workspace dir ─────────────────────────────
    var s1 = sr.beginStep("create-workspace");
    try {
        fs.mkdir(work);
        s1.assert("mkdir_no_throw", true);
        s1.assert("workspace_exists", fs.exists(work));
    } catch (e) {
        s1.assert("mkdir_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 2: Write report.md ──────────────────────────────────
    if (sr.hasFailed(1)) {
        sr.skipStep("write-report", 1);
    } else {
        var s2 = sr.beginStep("write-report");
        try {
            fs.writeFile(work + "/report.md", report_body);
            s2.assert("write_no_throw", true);
            s2.assert("report_exists", fs.exists(work + "/report.md"));
        } catch (e) {
            s2.assert("write_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 3: Read back ────────────────────────────────────────
    if (sr.hasFailed(2)) {
        sr.skipStep("read-report", 2);
    } else {
        var s3 = sr.beginStep("read-report");
        try {
            var got = fs.readFile(work + "/report.md");
            s3.assertEquals("content_matches", report_body, got);
        } catch (e) {
            s3.assert("read_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 4: List workspace ───────────────────────────────────
    if (sr.hasFailed(2)) {
        sr.skipStep("list-workspace", 2);
    } else {
        var s4 = sr.beginStep("list-workspace");
        try {
            var listing = fs.readDir(work);
            var names = normaliseFsListing(listing);
            s4.assert("report_in_listing", names.indexOf("report.md") !== -1,
                "names=" + JSON.stringify(names));
        } catch (e) {
            s4.assert("list_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 5: Write notes.md ───────────────────────────────────
    if (sr.hasFailed(1)) {
        sr.skipStep("write-notes", 1);
    } else {
        var s5 = sr.beginStep("write-notes");
        try {
            fs.writeFile(work + "/notes.md", notes_body);
            s5.assert("write_no_throw", true);
            s5.assert("notes_exists", fs.exists(work + "/notes.md"));
        } catch (e) {
            s5.assert("write_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 6: Search for keyword in report.md only (DOC-5) ────
    if (sr.hasFailed(2) || sr.hasFailed(5)) {
        sr.skipStep("search-finds-report-only", 5);
    } else {
        var s6 = sr.beginStep("search-finds-report-only");
        try {
            // fs.grep signature is (pattern, path), not (path, pattern).
            var hits = fs.grep("pineapple-42-unique", work);
            var rows = normaliseGrepRows(hits);
            var matchedPaths: string[] = [];
            for (var k = 0; k < rows.length; k++) {
                var r = rows[k];
                var p = typeof r === "string" ? r :
                        r.path ? r.path : r.file ? r.file : JSON.stringify(r);
                matchedPaths.push(p);
            }
            s6.assert("at_least_one_hit", rows.length >= 1,
                "matched=" + JSON.stringify(matchedPaths));
            var hitsReport = false, hitsNotes = false;
            for (var m = 0; m < matchedPaths.length; m++) {
                if (matchedPaths[m].indexOf("report.md") !== -1) hitsReport = true;
                if (matchedPaths[m].indexOf("notes.md")  !== -1) hitsNotes  = true;
            }
            s6.assert("report_hit", hitsReport);
            s6.assert("notes_not_hit", !hitsNotes);
        } catch (e) {
            s6.assert("search_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 7: Search for absent keyword — empty ────────────────
    if (sr.hasFailed(1)) {
        sr.skipStep("search-empty", 1);
    } else {
        var s7 = sr.beginStep("search-empty");
        try {
            var miss = fs.grep("jellyfish-xylophone-nonexistent", work);
            var missRows = normaliseGrepRows(miss);
            s7.assertEquals("search_empty_count", 0, missRows.length);
        } catch (e) {
            s7.assert("search_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 8: wc structured output (DOC-4) via rivers-exec ─────
    // wc-json.sh reads {"path":"..."} from stdin, emits
    // {"lines":N,"words":M,"bytes":K}. The exec driver's standard
    // output shape is {rows:[{result: <parsed-json>}], affected_rows:1}
    // — same as the integration_test.rs stdin echo case.
    if (sr.hasFailed(2)) {
        sr.skipStep("wc-structured-output", 2);
    } else {
        var s8 = sr.beginStep("wc-structured-output");
        try {
            var wcOut = ctx.dataview("exec_wc", {
                command: "wc",
                args: { path: work + "/report.md" }
            });
            var wcRows = (wcOut && wcOut.rows) ? wcOut.rows : [];
            s8.assertEquals("one_row", 1, wcRows.length);
            if (wcRows.length > 0) {
                // Result may be under `.result`, or fields may be at the row root
                // depending on how the exec driver surfaces JSON stdout.
                var parsed = wcRows[0].result ? wcRows[0].result : wcRows[0];
                s8.assert("lines_positive",
                    typeof parsed.lines === "number" && parsed.lines > 0,
                    "lines=" + JSON.stringify(parsed.lines));
                s8.assert("words_positive",
                    typeof parsed.words === "number" && parsed.words > 0);
                s8.assert("bytes_positive",
                    typeof parsed.bytes === "number" && parsed.bytes > 0);
            }
        } catch (e) {
            s8.assert("wc_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 9: Non-allowlisted command rejection (DOC-3) ───────
    // rivers-exec returns "unknown command: '<name>'" when the caller
    // names a command not in the hash-pinned allowlist. This is the
    // DOC-3 rejection path.
    var s9 = sr.beginStep("non-allowlisted-rejected");
    try {
        var threw9 = false;
        var err9 = "";
        try {
            ctx.dataview("exec_wc", {
                command: "this-command-is-not-allowlisted",
                args: {}
            });
        } catch (e) {
            threw9 = true;
            err9 = String(e);
        }
        s9.assert("rejected", threw9,
            threw9 ? ("threw: " + err9) : "did not throw");
        // Spec-implicit: error should name "unknown command" per the
        // exec driver test assertion (connection/mod.rs:225).
        if (threw9) {
            s9.assert("error_names_unknown_command",
                err9.indexOf("unknown command") !== -1
                || err9.indexOf("not allowlisted") !== -1
                || err9.indexOf("not in allowlist") !== -1,
                "error=" + err9);
        }
    } catch (e) {
        s9.assert("probe_no_throw", false, String(e));
    }
    sr.endStep();

    // ── Step 10: wc with path traversal rejected (DOC-6 exec side) ─
    // wc-json.sh performs the traversal check at the script layer
    // (rivers-exec has no path sandbox). The script returns exit 2
    // with {"error":"..."} JSON. How the exec driver surfaces that
    // varies — it may throw (caught below) or return a row with an
    // `error` field. Both shapes are accepted as rejection signals.
    if (sr.hasFailed(2)) {
        sr.skipStep("wc-path-traversal-rejected", 2);
    } else {
        var s10 = sr.beginStep("wc-path-traversal-rejected");
        try {
            var threw10 = false;
            var err10 = "";
            var travOut: any = null;
            try {
                travOut = ctx.dataview("exec_wc", {
                    command: "wc",
                    args: { path: "../../etc/passwd" }
                });
            } catch (e) {
                threw10 = true;
                err10 = String(e);
            }
            var rejectedViaThrow = threw10;
            var rejectedViaRow = false;
            if (!threw10 && travOut && travOut.rows && travOut.rows.length > 0) {
                var r = travOut.rows[0].result ? travOut.rows[0].result : travOut.rows[0];
                if (r && r.error) rejectedViaRow = true;
            }
            s10.assert("traversal_rejected",
                rejectedViaThrow || rejectedViaRow,
                "threw=" + threw10 + " rowError=" + rejectedViaRow
                + (threw10 ? " err=" + err10 : ""));
        } catch (e) {
            s10.assert("probe_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 11: Write with traversal filename rejected (DOC-6 fs side) ─
    if (sr.hasFailed(1)) {
        sr.skipStep("write-traversal-rejected", 1);
    } else {
        var s11 = sr.beginStep("write-traversal-rejected");
        try {
            // fs driver's chroot sandbox — spec says reject OR sanitize.
            // The canary-filesystem test (`fs_chroot_escape`) shows the
            // driver treats traversal as "not visible" (exists → false)
            // for exists/read, but we test writeFile here which should
            // throw.
            var threw = false;
            var errMsg = "";
            try {
                fs.writeFile(work + "/../../escape.md", "should-not-persist");
            } catch (e) {
                threw = true;
                errMsg = String(e);
            }
            s11.assert("traversal_write_rejected", threw,
                threw ? ("threw: " + errMsg) : "did not throw");
            // Confirm the would-be-escape file does NOT exist one level up.
            var escapeVisible = fs.exists("../escape.md");
            s11.assertEquals("escape_not_visible", false, escapeVisible);
        } catch (e) {
            s11.assert("traversal_probe_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 12: Delete notes.md ─────────────────────────────────
    if (sr.hasFailed(5)) {
        sr.skipStep("delete-notes", 5);
    } else {
        var s12 = sr.beginStep("delete-notes");
        try {
            fs.delete(work + "/notes.md");
            s12.assert("delete_no_throw", true);
            s12.assert("notes_gone", !fs.exists(work + "/notes.md"));
        } catch (e) {
            s12.assert("delete_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 13: List — only report.md remains ───────────────────
    if (sr.hasFailed(2)) {
        sr.skipStep("list-after-delete", 2);
    } else {
        var s13 = sr.beginStep("list-after-delete");
        try {
            var finalNames = normaliseFsListing(fs.readDir(work));
            s13.assert("report_still_there", finalNames.indexOf("report.md") !== -1,
                "names=" + JSON.stringify(finalNames));
            s13.assert("notes_gone", finalNames.indexOf("notes.md") === -1);
        } catch (e) {
            s13.assert("list_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Step 14: Stat report.md (DOC-10) ─────────────────────────
    if (sr.hasFailed(2)) {
        sr.skipStep("stat-report", 2);
    } else {
        var s14 = sr.beginStep("stat-report");
        try {
            var meta = fs.stat(work + "/report.md");
            s14.assertExists("stat_returned_object", meta);
            if (meta) {
                // Driver-dependent field names — accept size/bytes and mtime/modified.
                var hasSize = (typeof meta.size === "number" && meta.size > 0)
                           || (typeof meta.bytes === "number" && meta.bytes > 0);
                var hasTime = !!(meta.mtime || meta.modified || meta.modified_at || meta.mtime_ms);
                s14.assert("size_positive", hasSize, "meta=" + JSON.stringify(meta));
                s14.assert("mtime_present", hasTime, "meta=" + JSON.stringify(meta));
            }
        } catch (e) {
            s14.assert("stat_no_throw", false, String(e));
        }
        sr.endStep();
    }

    // ── Cleanup-after (best-effort, DOC-8) ───────────────────────
    try {
        if (fs.exists(work)) {
            var remainingNames = normaliseFsListing(fs.readDir(work));
            for (var p = 0; p < remainingNames.length; p++) {
                try { fs.delete(work + "/" + remainingNames[p]); } catch (_e) { /* ignore */ }
            }
            try { fs.delete(work); } catch (_e) { /* ignore */ }
        }
    } catch (e) {
        Rivers.log.warn("scenario-doc-pipeline cleanup-after failed: " + String(e));
    }

    ctx.resdata = sr.finish();
}
