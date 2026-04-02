(function() {
  "use strict";

  var PROFILES = [
    {
      name: "PROXY",
      endpoints: [
        { id: "proxy-health",   path: "/canary/proxy/health",   label: "Proxy Health" },
        { id: "proxy-guard",    path: "/canary/proxy/guard",    label: "Guard Forward" },
        { id: "proxy-sql",      path: "/canary/proxy/sql",      label: "SQL Forward" },
        { id: "proxy-nosql",    path: "/canary/proxy/nosql",    label: "NoSQL Forward" },
        { id: "proxy-handlers", path: "/canary/proxy/handlers", label: "Handlers Forward" },
        { id: "proxy-envelope", path: "/canary/proxy/envelope", label: "Response Envelope" }
      ]
    }
  ];

  function el(tag, attrs, children) {
    var node = document.createElement(tag);
    if (attrs) {
      Object.keys(attrs).forEach(function(k) {
        if (k === "className") node.className = attrs[k];
        else if (k === "textContent") node.textContent = attrs[k];
        else node.setAttribute(k, attrs[k]);
      });
    }
    if (children) {
      children.forEach(function(c) {
        if (typeof c === "string") node.appendChild(document.createTextNode(c));
        else node.appendChild(c);
      });
    }
    return node;
  }

  function clearNode(node) {
    while (node.firstChild) {
      node.removeChild(node.firstChild);
    }
  }

  function render() {
    var app = document.getElementById("app");
    clearNode(app);

    var header = el("header", { className: "header" }, [
      el("h1", { textContent: "Canary Fleet Dashboard" }),
      el("p", { className: "subtitle", textContent: "Cross-app proxy verification" })
    ]);
    app.appendChild(header);

    var statusBar = el("div", { id: "fleet-status", className: "status-bar status-pending" }, [
      el("span", { textContent: "Fleet Status: " }),
      el("span", { id: "fleet-status-text", textContent: "Running..." })
    ]);
    app.appendChild(statusBar);

    PROFILES.forEach(function(profile) {
      var section = el("section", { className: "profile" }, [
        el("h2", { textContent: profile.name + " Profile" })
      ]);

      var grid = el("div", { className: "test-grid" });

      profile.endpoints.forEach(function(ep) {
        var card = el("div", { id: "card-" + ep.id, className: "test-card pending" }, [
          el("div", { className: "test-label", textContent: ep.label }),
          el("div", { className: "test-status", id: "status-" + ep.id, textContent: "..." }),
          el("div", { className: "test-detail", id: "detail-" + ep.id })
        ]);
        grid.appendChild(card);
      });

      section.appendChild(grid);
      app.appendChild(section);
    });

    var runBtn = el("button", { id: "run-btn", className: "run-btn", textContent: "Run All Tests" });
    runBtn.addEventListener("click", runAll);
    app.appendChild(runBtn);
  }

  function runAll() {
    var btn = document.getElementById("run-btn");
    btn.disabled = true;
    btn.textContent = "Running...";

    var statusText = document.getElementById("fleet-status-text");
    var statusBar = document.getElementById("fleet-status");
    statusText.textContent = "Running...";
    statusBar.className = "status-bar status-pending";

    var total = 0;
    var passed = 0;
    var pending = 0;

    var allEndpoints = [];
    PROFILES.forEach(function(p) {
      p.endpoints.forEach(function(ep) { allEndpoints.push(ep); });
    });
    total = allEndpoints.length;
    pending = total;

    allEndpoints.forEach(function(ep) {
      var card = document.getElementById("card-" + ep.id);
      var statusEl = document.getElementById("status-" + ep.id);
      var detailEl = document.getElementById("detail-" + ep.id);

      card.className = "test-card pending";
      statusEl.textContent = "...";
      detailEl.textContent = "";

      fetch(ep.path)
        .then(function(res) { return res.json(); })
        .then(function(data) {
          var ok = data.passed === true;
          card.className = "test-card " + (ok ? "pass" : "fail");
          statusEl.textContent = ok ? "PASS" : "FAIL";

          if (data.assertions) {
            var summary = data.assertions.map(function(a) {
              return (a.passed ? "[ok]" : "[FAIL]") + " " + a.id;
            }).join(", ");
            detailEl.textContent = summary;
          }
          if (data.error) {
            detailEl.textContent = "Error: " + data.error;
          }

          if (ok) passed++;
          pending--;
          if (pending === 0) finalize();
        })
        .catch(function(err) {
          card.className = "test-card fail";
          statusEl.textContent = "ERROR";
          detailEl.textContent = String(err);

          pending--;
          if (pending === 0) finalize();
        });
    });

    function finalize() {
      var allPassed = passed === total;
      statusText.textContent = passed + " / " + total + " passed";
      statusBar.className = "status-bar " + (allPassed ? "status-pass" : "status-fail");
      btn.disabled = false;
      btn.textContent = "Run All Tests";
    }
  }

  render();
  runAll();
})();
