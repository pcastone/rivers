(function() {
  "use strict";

  // ── Configuration ──────────────────────────────────────────────
  var BASE = "/canary-fleet";
  var TEST_TIMEOUT = 15000;  // 15s per test (matches run-tests.sh)

  // ── Test Registry (matches run-tests.sh exactly) ───────────────
  var PROFILES = [
    {
      name: "AUTH",
      label: "Auth (Guard Login)",
      tests: [
        { id: "AUTH-GUARD-LOGIN", method: "POST", path: "/guard/canary/auth/login",
          body: '{"username":"canary","password":"canary-test"}', isGuard: true }
      ]
    },
    {
      name: "HANDLERS",
      label: "Handlers (Runtime)",
      tests: [
        { id: "RT-CTX-REQUEST",       method: "POST", path: "/handlers/canary/rt/ctx/request",        body: "{}" },
        { id: "RT-CTX-RESDATA",       method: "GET",  path: "/handlers/canary/rt/ctx/resdata" },
        { id: "RT-CTX-DATA",          method: "GET",  path: "/handlers/canary/rt/ctx/data" },
        { id: "RT-CTX-DATAVIEW",      method: "GET",  path: "/handlers/canary/rt/ctx/dataview" },
        { id: "RT-CTX-DATAVIEW-PARAMS", method: "POST", path: "/handlers/canary/rt/ctx/dataview-params", body: "{}" },
        { id: "RT-CTX-PSEUDO-DV",     method: "GET",  path: "/handlers/canary/rt/ctx/pseudo-dv" },
        { id: "RT-CTX-STORE-SET",     method: "POST", path: "/handlers/canary/rt/ctx/store",          body: "{}" },
        { id: "RT-CTX-STORE-GET",     method: "GET",  path: "/handlers/canary/rt/ctx/store" },
        { id: "RT-CTX-STORE-NS",      method: "GET",  path: "/handlers/canary/rt/ctx/store-ns" },
        { id: "RT-CTX-TRACE-ID",      method: "GET",  path: "/handlers/canary/rt/ctx/trace-id" },
        { id: "RT-CTX-NODE-ID",       method: "GET",  path: "/handlers/canary/rt/ctx/node-id" },
        { id: "RT-CTX-APP-ID",        method: "GET",  path: "/handlers/canary/rt/ctx/app-id" },
        { id: "RT-CTX-ENV",           method: "GET",  path: "/handlers/canary/rt/ctx/env" },
        { id: "RT-CTX-SESSION",       method: "GET",  path: "/handlers/canary/rt/ctx/session" },
        { id: "RT-CRYPTO-RANDOM",     method: "GET",  path: "/handlers/canary/rt/rivers/crypto-random" },
        { id: "RT-CRYPTO-HASH",       method: "GET",  path: "/handlers/canary/rt/rivers/crypto-hash" },
        { id: "RT-CRYPTO-TIMING",     method: "GET",  path: "/handlers/canary/rt/rivers/crypto-timing" },
        { id: "RT-CRYPTO-HMAC",       method: "GET",  path: "/handlers/canary/rt/rivers/crypto-hmac" },
        { id: "RT-RIVERS-LOG",        method: "GET",  path: "/handlers/canary/rt/rivers/log" },
        { id: "RT-V8-CODEGEN",        method: "GET",  path: "/handlers/canary/rt/v8/codegen" },
        { id: "RT-V8-CONSOLE",        method: "GET",  path: "/handlers/canary/rt/v8/console" },
        { id: "RT-ERROR-SANITIZE",    method: "GET",  path: "/handlers/canary/rt/error/sanitize" },
        { id: "RT-EVENTBUS-PUBLISH",  method: "POST", path: "/handlers/canary/rt/eventbus/publish",   body: "{}" },
        { id: "RT-HEADER-BLOCKLIST",  method: "GET",  path: "/handlers/canary/rt/header/blocklist" },
        { id: "RT-FAKER-DETERMINISM", method: "GET",  path: "/handlers/canary/rt/faker/determinism" }
      ]
    },
    {
      name: "SQL",
      label: "SQL (PG / MySQL / SQLite)",
      tests: [
        { id: "SQL-PG-PARAM-ORDER",    method: "POST",   path: "/sql/canary/sql/pg/param-order",    body: "{}" },
        { id: "SQL-PG-INSERT",          method: "POST",   path: "/sql/canary/sql/pg/insert",         body: "{}" },
        { id: "SQL-PG-SELECT",          method: "GET",    path: "/sql/canary/sql/pg/select" },
        { id: "SQL-PG-UPDATE",          method: "PUT",    path: "/sql/canary/sql/pg/update",         body: "{}" },
        { id: "SQL-PG-DELETE",          method: "DELETE",  path: "/sql/canary/sql/pg/delete",        body: "{}" },
        { id: "SQL-PG-DDL-REJECT",      method: "POST",   path: "/sql/canary/sql/pg/ddl-reject",    body: "{}" },
        { id: "SQL-PG-MAX-ROWS",        method: "GET",    path: "/sql/canary/sql/pg/max-rows" },
        { id: "SQL-MYSQL-PARAM-ORDER",  method: "POST",   path: "/sql/canary/sql/mysql/param-order", body: "{}" },
        { id: "SQL-MYSQL-INSERT",       method: "POST",   path: "/sql/canary/sql/mysql/insert",      body: "{}" },
        { id: "SQL-MYSQL-SELECT",       method: "GET",    path: "/sql/canary/sql/mysql/select" },
        { id: "SQL-MYSQL-UPDATE",       method: "PUT",    path: "/sql/canary/sql/mysql/update",      body: "{}" },
        { id: "SQL-MYSQL-DELETE",       method: "DELETE",  path: "/sql/canary/sql/mysql/delete",     body: "{}" },
        { id: "SQL-MYSQL-DDL-REJECT",   method: "POST",   path: "/sql/canary/sql/mysql/ddl-reject", body: "{}" },
        { id: "SQL-SQLITE-PARAM-ORDER", method: "GET",    path: "/sql/canary/sql/sqlite/param-order" },
        { id: "SQL-SQLITE-INSERT",      method: "POST",   path: "/sql/canary/sql/sqlite/insert",    body: "{}" },
        { id: "SQL-SQLITE-SELECT",      method: "GET",    path: "/sql/canary/sql/sqlite/select" },
        { id: "SQL-SQLITE-PREFIX",      method: "GET",    path: "/sql/canary/sql/sqlite/prefix" },
        { id: "SQL-CACHE-L1-HIT",       method: "GET",    path: "/sql/canary/sql/cache/l1-hit" },
        { id: "SQL-CACHE-INVALIDATE",   method: "POST",   path: "/sql/canary/sql/cache/invalidate", body: "{}" },
        { id: "SQL-INIT-DDL-SUCCESS",   method: "GET",    path: "/sql/canary/sql/init/ddl-success" },
        { id: "SQL-NEG-DDL-REJECTED",   method: "GET",    path: "/sql/canary/sql/negative/ddl-rejected" },
        { id: "SQL-NEG-ERROR-SANITIZED", method: "GET",   path: "/sql/canary/sql/negative/error-sanitized" }
      ]
    },
    {
      name: "NoSQL",
      label: "NoSQL (Mongo / ES / Couch / Cassandra / LDAP / Redis)",
      tests: [
        { id: "NOSQL-MONGO-PING",         method: "GET",  path: "/nosql/canary/nosql/mongo/ping" },
        { id: "NOSQL-MONGO-INSERT",        method: "POST", path: "/nosql/canary/nosql/mongo/insert",       body: "{}" },
        { id: "NOSQL-MONGO-FIND",          method: "GET",  path: "/nosql/canary/nosql/mongo/find" },
        { id: "NOSQL-MONGO-ADMIN-REJECT",  method: "POST", path: "/nosql/canary/nosql/mongo/admin-reject", body: "{}" },
        { id: "NOSQL-ES-PING",            method: "GET",  path: "/nosql/canary/nosql/es/ping" },
        { id: "NOSQL-ES-INDEX",           method: "POST", path: "/nosql/canary/nosql/es/index",            body: "{}" },
        { id: "NOSQL-ES-SEARCH",          method: "GET",  path: "/nosql/canary/nosql/es/search" },
        { id: "NOSQL-COUCH-PING",         method: "GET",  path: "/nosql/canary/nosql/couch/ping" },
        { id: "NOSQL-COUCH-PUT",          method: "POST", path: "/nosql/canary/nosql/couch/put",           body: "{}" },
        { id: "NOSQL-COUCH-GET",          method: "GET",  path: "/nosql/canary/nosql/couch/get" },
        { id: "NOSQL-CASSANDRA-PING",     method: "GET",  path: "/nosql/canary/nosql/cassandra/ping" },
        { id: "NOSQL-CASSANDRA-INSERT",   method: "POST", path: "/nosql/canary/nosql/cassandra/insert",    body: "{}" },
        { id: "NOSQL-CASSANDRA-SELECT",   method: "GET",  path: "/nosql/canary/nosql/cassandra/select" },
        { id: "NOSQL-LDAP-PING",          method: "GET",  path: "/nosql/canary/nosql/ldap/ping" },
        { id: "NOSQL-LDAP-SEARCH",        method: "GET",  path: "/nosql/canary/nosql/ldap/search" },
        { id: "NOSQL-REDIS-PING",         method: "GET",  path: "/nosql/canary/nosql/redis/ping" },
        { id: "NOSQL-REDIS-SET",          method: "POST", path: "/nosql/canary/nosql/redis/set",           body: "{}" },
        { id: "NOSQL-REDIS-GET",          method: "GET",  path: "/nosql/canary/nosql/redis/get" },
        { id: "NOSQL-REDIS-ADMIN-REJECT", method: "POST", path: "/nosql/canary/nosql/redis/admin-reject",  body: "{}" }
      ]
    },
    {
      name: "STREAMS",
      label: "Streams",
      tests: [
        { id: "STREAM-POLL-HASH", method: "GET", path: "/streams/canary/stream/poll/data" }
      ]
    },
    {
      name: "V8",
      label: "V8 Security (slow)",
      tests: [
        { id: "RT-V8-TIMEOUT", method: "GET", path: "/handlers/canary/rt/v8/timeout", isV8Sec: true },
        { id: "RT-V8-HEAP",    method: "GET", path: "/handlers/canary/rt/v8/heap",    isV8Sec: true }
      ]
    },
    {
      // CS5 — scenario verdicts surface alongside atomic tests.
      // The envelope is type="scenario" with per-step assertions[]; the
      // existing renderer picks up top-level `passed` and aggregates
      // assertions across all steps for the expand-card view. Per-step
      // visual breakdown (expand/collapse with failed_at_step banner) is
      // a follow-on polish; scenario assertions render correctly via the
      // existing assertion list today.
      name: "SCENARIOS",
      label: "Scenarios (multi-step)",
      tests: [
        { id: "SCEN-SQL-PROBE",            method: "POST", path: "/sql/canary/scenarios/sql/probe",            body: "{}" },
        { id: "SCEN-STREAM-PROBE",         method: "POST", path: "/streams/canary/scenarios/stream/probe",     body: "{}" },
        { id: "SCEN-RUNTIME-PROBE",        method: "POST", path: "/handlers/canary/scenarios/runtime/probe",   body: "{}" },
        { id: "SCEN-SQL-MESSAGING-SQLITE", method: "POST", path: "/sql/canary/scenarios/sql/messaging/sqlite", body: "{}" },
        { id: "SCEN-SQL-MESSAGING-PG",     method: "POST", path: "/sql/canary/scenarios/sql/messaging/pg",     body: "{}" },
        { id: "SCEN-SQL-MESSAGING-MYSQL",  method: "POST", path: "/sql/canary/scenarios/sql/messaging/mysql",  body: "{}" },
        { id: "SCEN-RUNTIME-DOC-PIPELINE", method: "POST", path: "/handlers/canary/scenarios/runtime/doc-pipeline", body: "{}" },
        // CS3 / BR5 — Activity Feed (unblocked 2026-04-23 by broker-publish bridge).
        { id: "SCEN-STREAM-ACTIVITY-FEED", method: "POST", path: "/streams/canary/scenarios/stream/activity-feed", body: "{}" }
      ]
    }
  ];

  // ── State ──────────────────────────────────────────────────────

  var state = {
    serverStatus: "pending",   // pending | connected | disconnected
    csrfToken: "",
    running: false,
    results: {},               // test_id -> { status, data, duration, raw, httpStatus }
    expandedCard: null,
    openProfiles: {},          // profile_name -> true/false
    rawVisible: {}             // test_id -> true/false
  };

  // Initialize all profiles as open
  PROFILES.forEach(function(p) { state.openProfiles[p.name] = true; });

  // ── Helpers ────────────────────────────────────────────────────

  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    if (attrs) {
      Object.keys(attrs).forEach(function(k) {
        if (k === "className") node.className = attrs[k];
        else if (k === "textContent") node.textContent = attrs[k];
        else if (k.indexOf("on") === 0) node.addEventListener(k.slice(2), attrs[k]);
        else node.setAttribute(k, attrs[k]);
      });
    }
    if (children) {
      children.forEach(function(c) {
        if (typeof c === "string") node.appendChild(document.createTextNode(c));
        else if (c) node.appendChild(c);
      });
    }
    return node;
  }

  function getCounts() {
    var pass = 0, fail = 0, error = 0, running = 0, total = 0;
    PROFILES.forEach(function(p) {
      p.tests.forEach(function(t) {
        total++;
        var r = state.results[t.id];
        if (!r) return;
        if (r.status === "pass") pass++;
        else if (r.status === "fail") fail++;
        else if (r.status === "error" || r.status === "timeout") error++;
        else if (r.status === "running") running++;
      });
    });
    return { pass: pass, fail: fail, error: error, running: running, total: total,
             done: pass + fail + error };
  }

  function getProfileCounts(profile) {
    var pass = 0, fail = 0, error = 0, total = profile.tests.length;
    profile.tests.forEach(function(t) {
      var r = state.results[t.id];
      if (!r) return;
      if (r.status === "pass") pass++;
      else if (r.status === "fail") fail++;
      else if (r.status === "error" || r.status === "timeout") error++;
    });
    return { pass: pass, fail: fail, error: error, total: total };
  }

  // ── Fetch with timeout ─────────────────────────────────────────

  function fetchWithTimeout(url, opts, timeout) {
    return new Promise(function(resolve, reject) {
      var timer = setTimeout(function() {
        reject(new Error("TIMEOUT"));
      }, timeout);

      fetch(url, opts).then(function(res) {
        clearTimeout(timer);
        resolve(res);
      }).catch(function(err) {
        clearTimeout(timer);
        reject(err);
      });
    });
  }

  // ── Auth: login to get session cookie + CSRF ───────────────────

  function doLogin() {
    return fetchWithTimeout(BASE + "/guard/canary/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      credentials: "same-origin",
      body: '{"username":"canary","password":"canary-test"}'
    }, 8000).then(function(res) {
      return res.json().then(function(data) {
        if (data.allow) {
          state.serverStatus = "connected";
          // Extract CSRF token from cookies
          var cookies = document.cookie.split(";");
          for (var i = 0; i < cookies.length; i++) {
            var c = cookies[i].trim();
            if (c.indexOf("rivers_csrf=") === 0) {
              state.csrfToken = c.substring(12);
              break;
            }
          }
          state.results["AUTH-GUARD-LOGIN"] = {
            status: "pass", data: data, duration: null, raw: JSON.stringify(data, null, 2),
            httpStatus: res.status
          };
        } else {
          state.results["AUTH-GUARD-LOGIN"] = {
            status: "fail", data: data, duration: null, raw: JSON.stringify(data, null, 2),
            httpStatus: res.status
          };
        }
      });
    }).catch(function(err) {
      state.serverStatus = "disconnected";
      state.results["AUTH-GUARD-LOGIN"] = {
        status: "error", data: null, duration: null, raw: String(err), httpStatus: 0
      };
    });
  }

  // ── Run a single test ──────────────────────────────────────────

  function runTest(test) {
    if (test.isGuard) {
      state.results[test.id] = { status: "running" };
      render();
      return doLogin().then(function() { render(); });
    }

    state.results[test.id] = { status: "running" };
    render();

    var opts = {
      method: test.method,
      credentials: "same-origin",
      headers: {}
    };

    if (test.body) {
      opts.headers["Content-Type"] = "application/json";
      opts.body = test.body;
    }

    if (test.method !== "GET" && state.csrfToken) {
      opts.headers["X-CSRF-Token"] = state.csrfToken;
    }

    var startTime = Date.now();
    var timeout = test.isV8Sec ? TEST_TIMEOUT : 8000;

    return fetchWithTimeout(BASE + test.path, opts, timeout).then(function(res) {
      var duration = Date.now() - startTime;
      var httpStatus = res.status;

      return res.json().then(function(data) {
        var passed = data.passed === true;

        // V8 security: HTTP 500 = server survived = PASS
        if (!passed && test.isV8Sec && httpStatus === 500) {
          passed = true;
        }

        state.results[test.id] = {
          status: passed ? "pass" : "fail",
          data: data,
          duration: duration,
          raw: JSON.stringify(data, null, 2),
          httpStatus: httpStatus
        };
      }).catch(function() {
        // Non-JSON response
        var duration2 = Date.now() - startTime;
        if (test.isV8Sec && httpStatus === 500) {
          state.results[test.id] = {
            status: "pass", data: null, duration: duration2,
            raw: "(non-JSON 500 response -- server survived)", httpStatus: httpStatus
          };
        } else {
          state.results[test.id] = {
            status: "error", data: null, duration: duration2,
            raw: "HTTP " + httpStatus + " (non-JSON response)", httpStatus: httpStatus
          };
        }
      });
    }).catch(function(err) {
      var duration = Date.now() - startTime;
      var isTimeout = String(err).indexOf("TIMEOUT") >= 0;
      state.results[test.id] = {
        status: isTimeout ? "timeout" : "error",
        data: null,
        duration: duration,
        raw: String(err),
        httpStatus: 0
      };
    }).then(function() {
      render();
    });
  }

  // ── Run a list of tests sequentially ───────────────────────────

  function runSequential(tests) {
    state.running = true;
    render();

    var chain = Promise.resolve();
    tests.forEach(function(t) {
      chain = chain.then(function() { return runTest(t); });
    });

    return chain.then(function() {
      state.running = false;
      render();
    });
  }

  // ── Run All ────────────────────────────────────────────────────

  function runAll() {
    if (state.running) return;

    // Reset all results
    state.results = {};
    state.expandedCard = null;
    state.rawVisible = {};

    // Warm up V8 (matches run-tests.sh)
    var warmup = fetchWithTimeout(BASE + "/handlers/canary/rt/ctx/trace-id", {
      credentials: "same-origin"
    }, 5000).catch(function() {});

    warmup.then(function() {
      // Collect all tests in order
      var allTests = [];
      PROFILES.forEach(function(p) {
        p.tests.forEach(function(t) { allTests.push(t); });
      });
      return runSequential(allTests);
    });
  }

  // ── Run Profile ────────────────────────────────────────────────

  function runProfile(profile) {
    if (state.running) return;

    // Reset only this profile's results
    profile.tests.forEach(function(t) { delete state.results[t.id]; });
    state.expandedCard = null;

    // If AUTH profile, just run it; otherwise ensure we're logged in first
    if (profile.name === "AUTH") {
      return runSequential(profile.tests);
    }

    var loginFirst = state.serverStatus !== "connected"
      ? doLogin().then(function() { render(); })
      : Promise.resolve();

    return loginFirst.then(function() {
      return runSequential(profile.tests);
    });
  }

  // ── Render helpers for profile count spans ─────────────────────

  function buildCountSpan(pc) {
    var span = el("span", { className: "profile-count" });
    var parts = [];

    if (pc.fail > 0) {
      parts.push(document.createTextNode("("));
      var passSpan = el("span", { className: "pass-num", textContent: String(pc.pass) });
      parts.push(passSpan);
      parts.push(document.createTextNode("/"));
      var failSpan = el("span", { className: "fail-num", textContent: pc.fail + " fail" });
      parts.push(failSpan);
      parts.push(document.createTextNode("/" + pc.total + ")"));
    } else if (pc.pass > 0) {
      parts.push(document.createTextNode("("));
      var passSpan2 = el("span", { className: "pass-num", textContent: String(pc.pass) });
      parts.push(passSpan2);
      parts.push(document.createTextNode("/" + pc.total + ")"));
    } else {
      parts.push(document.createTextNode("(0/" + pc.total + ")"));
    }

    parts.forEach(function(p) { span.appendChild(p); });
    return span;
  }

  // ── Render ─────────────────────────────────────────────────────

  function render() {
    var app = document.getElementById("app");
    // Remove all children
    while (app.firstChild) app.removeChild(app.firstChild);

    var counts = getCounts();

    // Header
    var statusClass = state.serverStatus === "connected" ? "connected"
      : state.serverStatus === "disconnected" ? "disconnected" : "pending";
    var statusLabel = state.serverStatus === "connected" ? "Connected"
      : state.serverStatus === "disconnected" ? "Disconnected" : "...";

    app.appendChild(el("header", { className: "header" }, [
      el("h1", { textContent: "Canary Fleet Dashboard" }),
      el("p", { className: "subtitle", textContent: "70 tests across 6 profiles" }),
      el("span", { className: "server-status " + statusClass, textContent: statusLabel })
    ]));

    // Summary bar
    var barClass = state.running ? "running"
      : counts.done === 0 ? "idle"
      : counts.fail + counts.error === 0 ? "all-pass"
      : "has-fail";

    var progressPct = counts.total > 0 ? Math.round((counts.done / counts.total) * 100) : 0;

    var summaryLeft = el("div", { className: "summary-counts" }, [
      el("span", { className: "count-pass", textContent: counts.pass + " pass" }),
      el("span", { className: "count-fail", textContent: counts.fail + " fail" }),
      el("span", { className: "count-error", textContent: counts.error + " err" }),
      el("span", { className: "count-total", textContent: counts.done + "/" + counts.total })
    ]);

    var progressBar = el("div", { className: "summary-progress" }, [
      el("div", { className: "progress-fill", style: "width:" + progressPct + "%" })
    ]);

    var runAllBtn = el("button", {
      className: "btn btn-primary",
      textContent: state.running ? "Running..." : "Run All",
      onclick: function() { runAll(); }
    });
    if (state.running) runAllBtn.disabled = true;

    app.appendChild(el("div", { className: "summary-bar " + barClass }, [
      summaryLeft, progressBar, runAllBtn
    ]));

    // Profile sections
    PROFILES.forEach(function(profile) {
      var pc = getProfileCounts(profile);
      var isOpen = state.openProfiles[profile.name];

      var profileDiv = el("div", { className: "profile" + (isOpen ? " open" : "") });

      // Header
      var countSpan = buildCountSpan(pc);

      var profileLeft = el("div", { className: "profile-left" }, [
        el("span", { className: "profile-chevron", textContent: "\u25B6" }),
        el("span", { className: "profile-name", textContent: profile.name }),
        countSpan
      ]);

      var runProfileBtn = el("button", {
        className: "btn btn-profile",
        textContent: "Run " + profile.name,
        onclick: function(e) { e.stopPropagation(); runProfile(profile); }
      });
      if (state.running) runProfileBtn.disabled = true;

      var profileRight = el("div", { className: "profile-right" }, [runProfileBtn]);

      var headerDiv = el("div", { className: "profile-header" });
      headerDiv.appendChild(profileLeft);
      headerDiv.appendChild(profileRight);
      (function(pName) {
        headerDiv.addEventListener("click", function() {
          state.openProfiles[pName] = !state.openProfiles[pName];
          render();
        });
      })(profile.name);
      profileDiv.appendChild(headerDiv);

      // Body (test cards)
      var body = el("div", { className: "profile-body" });
      var grid = el("div", { className: "test-grid" });

      profile.tests.forEach(function(test) {
        var r = state.results[test.id];
        var cardStatus = r ? r.status : "pending";
        var isExpanded = state.expandedCard === test.id;

        var card = el("div", {
          className: "test-card " + cardStatus + (isExpanded ? " expanded" : "")
        });

        // Click to expand/collapse
        (function(tId) {
          card.addEventListener("click", function() {
            var result = state.results[tId];
            if (result) {
              state.expandedCard = state.expandedCard === tId ? null : tId;
              render();
            }
          });
        })(test.id);

        // Top row: test ID + status dot
        card.appendChild(el("div", { className: "test-card-top" }, [
          el("span", { className: "test-id", textContent: test.id }),
          el("span", { className: "test-status-dot" })
        ]));

        // Duration
        if (r && r.duration) {
          card.appendChild(el("div", { className: "test-duration", textContent: r.duration + "ms" }));
        }

        // Run button (individual, appears on hover)
        (function(t) {
          var runBtn = el("button", {
            className: "test-run-btn",
            textContent: "\u25B6"
          });
          runBtn.addEventListener("click", function(e) {
            e.stopPropagation();
            if (!state.running) {
              if (!t.isGuard && state.serverStatus !== "connected") {
                doLogin().then(function() { render(); return runTest(t); });
              } else {
                runTest(t);
              }
            }
          });
          card.appendChild(runBtn);
        })(test);

        // Expanded detail panel
        if (r && isExpanded) {
          var detail = el("div", { className: "test-detail" });
          detail.style.display = "block";

          // Assertions
          if (r.data && r.data.assertions) {
            r.data.assertions.forEach(function(a) {
              var children = [
                el("span", {
                  className: a.passed ? "assertion-pass" : "assertion-fail",
                  textContent: a.passed ? "\u2713" : "\u2717"
                }),
                el("span", { textContent: a.id || a.name || "assertion" })
              ];
              if (a.detail) {
                children.push(el("span", { className: "assertion-detail", textContent: a.detail }));
              }
              detail.appendChild(el("div", { className: "assertion-row" }, children));
            });
          }

          // Error message
          if (r.data && r.data.error) {
            detail.appendChild(el("div", { className: "test-error", textContent: r.data.error }));
          }
          if (r.status === "timeout") {
            detail.appendChild(el("div", { className: "test-error", textContent: "Request timed out" }));
          }

          // V8 sec note
          if (r.httpStatus === 500 && r.status === "pass") {
            detail.appendChild(el("div", {
              className: "test-duration",
              textContent: "HTTP 500 -- server survived (expected for V8 security tests)"
            }));
          }

          // Raw JSON toggle
          var isRawVisible = state.rawVisible[test.id];
          (function(tId) {
            var rawToggle = el("div", {
              className: "test-raw-toggle",
              textContent: isRawVisible ? "Hide raw JSON" : "Show raw JSON"
            });
            rawToggle.addEventListener("click", function(e) {
              e.stopPropagation();
              state.rawVisible[tId] = !state.rawVisible[tId];
              render();
            });
            detail.appendChild(rawToggle);
          })(test.id);

          if (isRawVisible) {
            detail.appendChild(el("pre", {
              className: "test-raw visible",
              textContent: r.raw || "(no data)"
            }));
          }

          card.appendChild(detail);
        }

        grid.appendChild(card);
      });

      body.appendChild(grid);
      profileDiv.appendChild(body);
      app.appendChild(profileDiv);
    });

    // Timestamp
    if (counts.done > 0 && !state.running) {
      app.appendChild(el("div", {
        className: "run-timestamp",
        textContent: "Last run: " + new Date().toLocaleString()
      }));
    }
  }

  // ── Init ───────────────────────────────────────────────────────

  render();

})();
