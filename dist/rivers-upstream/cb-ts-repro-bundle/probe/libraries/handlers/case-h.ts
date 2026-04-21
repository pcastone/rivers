// Case H — the working TS shape in Rivers v0.54.1.
// Uses only stripper-supported constructs:
//   - interface declaration (stripped)
//   - type alias `=` declaration (stripped)
//   - return type annotation `): T {` → `) {` (stripped)
//   - `as Type` assertion (stripped)
// Avoids:
//   - parameter annotations
//   - variable annotations
//   - generics
//   - import/export
//
// Expected: 200 { "case": "H", "outcome": "pass", ... }

interface Response {
    case: string;
    outcome: string;
    n: number;
}

type Digit = number;

function makeResponse(n): Response {
    return { case: "H", outcome: "pass", n: n as Digit };
}

function handler(ctx) {
    ctx.resdata = makeResponse(42);
}
