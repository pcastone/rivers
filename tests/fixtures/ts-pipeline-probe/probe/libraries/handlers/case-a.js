// Case A — baseline: plain JS, no types, no imports.
// Expected: 200 { "case": "A", "outcome": "pass", "note": "..." }

function handler(ctx) {
    ctx.resdata = {
        case: "A",
        outcome: "pass",
        note: "plain-JS baseline; no TS stripper or module-mode involvement"
    };
}
