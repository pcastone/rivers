// Case B — parameter type annotation.
// Docstring at v8_config.rs:124 claims `(x: string) -> (x)`. Not implemented.
// Expected per docstring: 200 pass. Actual: 500 SyntaxError.
//
// Stripper output (verbatim input, no changes):
//     function handler(ctx: any) { ... }
// V8 classic script parse:
//     SyntaxError: Unexpected identifier 'any'

function handler(ctx: any) {
    ctx.resdata = {
        case: "B",
        outcome: "pass",
        note: "parameter type annotation was stripped"
    };
}
