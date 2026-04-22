// FILESYSTEM profile test handlers — exercise the typed-proxy surface
// (ctx.datasource("canary-fs").<op>) for a chroot-sandboxed filesystem
// datasource rooted at /tmp. Every handler confines its writes to a
// dedicated work directory it creates and removes itself.
//
// Each handler returns a TestResult via ctx.resdata so run-tests.sh can
// aggregate pass/fail counts.

// ── Test harness (mirrors canary-sql pattern) ──

function TestResult(test_id, profile, spec_ref) {
    this.test_id = test_id;
    this.profile = profile;
    this.spec_ref = spec_ref;
    this.assertions = [];
    this.error = null;
    this.start = Date.now();
}
TestResult.prototype.assert = function(id, passed, detail) {
    this.assertions.push({ id: id, passed: !!passed, detail: detail || undefined });
};
TestResult.prototype.assertEquals = function(id, expected, actual) {
    var passed = JSON.stringify(expected) === JSON.stringify(actual);
    this.assertions.push({
        id: id, passed: passed,
        detail: passed ? "expected=" + JSON.stringify(expected)
            : "expected=" + JSON.stringify(expected) + ", actual=" + JSON.stringify(actual)
    });
};
TestResult.prototype.finish = function() {
    return {
        test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: this.assertions.every(function(a) { return a.passed; }),
        assertions: this.assertions, duration_ms: Date.now() - this.start, error: this.error
    };
};
TestResult.prototype.fail = function(err) {
    this.error = err;
    return {
        test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: false, assertions: this.assertions, duration_ms: Date.now() - this.start, error: err
    };
};

// Per-test work dir under /tmp. Each handler owns one.
function workDir(test_id) {
    return "rivers-canary-fs-" + test_id;
}

function cleanup(fs, dir) {
    try { fs.delete(dir); } catch (_e) { /* idempotent */ }
}

// ── FS-CRUD-ROUNDTRIP — write → read → delete ──

function fsCrudRoundtrip(ctx) {
    var t = new TestResult("FS-CRUD-ROUNDTRIP", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 4");
    var fs = ctx.datasource("canary-fs");
    var work = workDir("crud");
    cleanup(fs, work);
    try {
        fs.mkdir(work);
        fs.writeFile(work + "/hello.txt", "world");
        var got = fs.readFile(work + "/hello.txt");
        t.assertEquals("read_after_write", "world", got);

        // base64 round-trip (binary safety)
        fs.writeFile(work + "/bin.dat", "/wD+", "base64");
        var b64 = fs.readFile(work + "/bin.dat", "base64");
        t.assertEquals("base64_roundtrip", "/wD+", b64);

        // rename + copy
        fs.rename(work + "/hello.txt", work + "/renamed.txt");
        t.assert("rename_source_gone", !fs.exists(work + "/hello.txt"));
        t.assert("rename_target_present", fs.exists(work + "/renamed.txt"));

        fs.copy(work + "/renamed.txt", work + "/copied.txt");
        t.assertEquals("copy_content_matches", "world", fs.readFile(work + "/copied.txt"));
    } catch (e) {
        cleanup(fs, work);
        ctx.resdata = t.fail(String(e));
        return;
    }
    cleanup(fs, work);
    ctx.resdata = t.finish();
}

// ── FS-CHROOT-ESCAPE — ../ and absolute paths must be rejected ──

function fsChrootEscape(ctx) {
    var t = new TestResult("FS-CHROOT-ESCAPE", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 5");
    var fs = ctx.datasource("canary-fs");

    // Absolute path — driver rejects with DriverError::Query
    try {
        fs.readFile("/etc/passwd");
        t.assert("absolute_path_rejected", false, "readFile('/etc/passwd') did not throw");
    } catch (_e) {
        t.assert("absolute_path_rejected", true);
    }

    // Traversal — exists() returns false (spec: escape → not visible, not error)
    var visible = fs.exists("../../etc/passwd");
    t.assertEquals("traversal_exists_false", false, visible);

    ctx.resdata = t.finish();
}

// ── FS-EXISTS-AND-STAT — metadata ops ──

function fsExistsAndStat(ctx) {
    var t = new TestResult("FS-EXISTS-AND-STAT", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 4");
    var fs = ctx.datasource("canary-fs");
    var work = workDir("stat");
    cleanup(fs, work);
    try {
        fs.mkdir(work);
        fs.writeFile(work + "/f.txt", "12345");

        t.assertEquals("exists_true_for_file", true, fs.exists(work + "/f.txt"));
        t.assertEquals("exists_false_for_missing", false, fs.exists(work + "/nope.txt"));

        var info = fs.stat(work + "/f.txt");
        t.assertEquals("stat_size_matches", 5, info.size);
        t.assertEquals("stat_isFile", true, info.isFile);
        t.assertEquals("stat_isDirectory", false, info.isDirectory);

        var dir_info = fs.stat(work);
        t.assertEquals("stat_dir_isDirectory", true, dir_info.isDirectory);
    } catch (e) {
        cleanup(fs, work);
        ctx.resdata = t.fail(String(e));
        return;
    }
    cleanup(fs, work);
    ctx.resdata = t.finish();
}

// ── FS-FIND-AND-GREP — glob + regex with truncation + binary skip ──

function fsFindAndGrep(ctx) {
    var t = new TestResult("FS-FIND-AND-GREP", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 4");
    var fs = ctx.datasource("canary-fs");
    var work = workDir("search");
    cleanup(fs, work);
    try {
        fs.mkdir(work);
        for (var i = 0; i < 4; i++) {
            fs.writeFile(work + "/note-" + i + ".txt", "TODO: item " + i);
        }
        fs.writeFile(work + "/note-binary.bin", "/wD+", "base64");

        // find with max_results truncation
        var found = fs.find(work + "/*.txt", 2);
        t.assertEquals("find_truncated_len", 2, found.results.length);
        t.assertEquals("find_truncated_flag", true, found.truncated);

        // find default limit (no truncation for 4 files)
        var all = fs.find(work + "/*.txt");
        t.assertEquals("find_all_len", 4, all.results.length);
        t.assertEquals("find_all_not_truncated", false, all.truncated);

        // grep matches TODO in text files, skips the binary
        var grep = fs.grep("TODO", work);
        t.assertEquals("grep_hit_count", 4, grep.results.length);
        t.assertEquals("grep_not_truncated", false, grep.truncated);
    } catch (e) {
        cleanup(fs, work);
        ctx.resdata = t.fail(String(e));
        return;
    }
    cleanup(fs, work);
    ctx.resdata = t.finish();
}

// ── FS-ARG-VALIDATION — typed proxy rejects wrong shapes before dispatch ──

function fsArgValidation(ctx) {
    var t = new TestResult("FS-ARG-VALIDATION", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 3.3");
    var fs = ctx.datasource("canary-fs");

    // readFile(42) — path must be string
    try {
        fs.readFile(42);
        t.assert("nonstring_path_rejected", false, "readFile(42) did not throw");
    } catch (e) {
        t.assert("nonstring_path_rejected",
            String(e.message || e).indexOf("must be a string") !== -1,
            "msg=" + String(e.message || e));
    }

    // readFile() — required param missing
    try {
        fs.readFile();
        t.assert("missing_required_rejected", false, "readFile() did not throw");
    } catch (e) {
        t.assert("missing_required_rejected",
            String(e.message || e).indexOf("is required") !== -1,
            "msg=" + String(e.message || e));
    }

    // find('*.txt', 'ten') — max_results must be integer
    try {
        fs.find("*.txt", "ten");
        t.assert("noninteger_max_rejected", false, "find(..., 'ten') did not throw");
    } catch (e) {
        t.assert("noninteger_max_rejected",
            String(e.message || e).indexOf("must be a integer") !== -1,
            "msg=" + String(e.message || e));
    }

    ctx.resdata = t.finish();
}

// ── FS-READ-DIR — readDir returns {name:string}[] objects ──

function fsReadDir(ctx) {
    var t = new TestResult("FS-READ-DIR", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 6.4");
    var fs = ctx.datasource("canary-fs");
    var work = workDir("readdir");
    cleanup(fs, work);
    try {
        fs.mkdir(work);
        fs.writeFile(work + "/alpha.txt", "a");
        fs.writeFile(work + "/beta.txt", "b");
        fs.mkdir(work + "/subdir");

        var entries = fs.readDir(work);

        // Must be an array
        t.assert("is_array", Array.isArray(entries), "readDir returned: " + typeof entries);

        // Each entry must be an object with a name property
        t.assert("entries_have_name",
            entries.every(function(e) { return typeof e === 'object' && typeof e.name === 'string'; }),
            "entry shape wrong: " + JSON.stringify(entries[0]));

        // Must contain our three entries (order not guaranteed)
        var names = entries.map(function(e) { return e.name; }).sort();
        t.assertEquals("entry_count", 3, names.length);
        t.assertEquals("entry_alpha", "alpha.txt", names[0]);
        t.assertEquals("entry_beta", "beta.txt", names[1]);
        t.assertEquals("entry_subdir", "subdir", names[2]);

        // Must not include . or ..
        t.assert("no_dot_entries",
            !names.some(function(n) { return n === '.' || n === '..'; }));

    } catch (e) {
        cleanup(fs, work);
        ctx.resdata = t.fail(String(e));
        return;
    }
    cleanup(fs, work);
    ctx.resdata = t.finish();
}

// ── FS-CONCURRENT-WRITES — N independent write+read sequences in one handler ──
// V8 is single-threaded per isolate — "concurrent" here means multiple
// independent write sequences with no shared state between them.
// Validates the driver's lock-free model (each op is path-independent).

function fsConcurrentWrites(ctx) {
    var t = new TestResult("FS-CONCURRENT-WRITES", "FILESYSTEM", "rivers-filesystem-driver-spec.md section 5.4");
    var fs = ctx.datasource("canary-fs");
    var N = 8;
    var dirs = [];
    for (var i = 0; i < N; i++) {
        dirs.push(workDir("concurrent-" + i));
    }
    dirs.forEach(function(d) { cleanup(fs, d); });

    try {
        // Write phase — each dir is independent, no shared state
        for (var i = 0; i < N; i++) {
            fs.mkdir(dirs[i]);
            fs.writeFile(dirs[i] + "/data.txt", "payload-" + i);
        }

        // Read-back phase — each file must contain exactly its own payload
        var allCorrect = true;
        for (var i = 0; i < N; i++) {
            var got = fs.readFile(dirs[i] + "/data.txt");
            if (got !== "payload-" + i) {
                allCorrect = false;
                t.assert("payload_" + i, false,
                    "expected 'payload-" + i + "', got '" + got + "'");
            } else {
                t.assert("payload_" + i, true);
            }
        }
        t.assert("all_payloads_correct", allCorrect);

    } catch (e) {
        dirs.forEach(function(d) { cleanup(fs, d); });
        ctx.resdata = t.fail(String(e));
        return;
    }
    dirs.forEach(function(d) { cleanup(fs, d); });
    ctx.resdata = t.finish();
}
