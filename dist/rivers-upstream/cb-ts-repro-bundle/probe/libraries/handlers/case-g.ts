// Case G — single-file ES module with `export function`.
// is_module_syntax() detects `export `; execute_as_module() runs; entrypoint
// ends up on the module namespace, not globalThis. call_entrypoint("handler")
// looks in the global scope and fails.
//
// Comment at execution.rs:222-224 acknowledges:
//   "For V1: the module must set the entrypoint on the global scope
//    (e.g., via a side effect or `globalThis.handler = handler`)."
//
// Expected: 500 "entrypoint function 'handler' not found on global scope"
// (or similar, once instantiation succeeds).

export function handler(ctx) {
    ctx.resdata = { case: "G", outcome: "pass" };
}
