// Case D — `type`-only import inside a named-imports list.
// The TS stripper has no pattern for `type` inside an import; leaves the
// `type X` token in place. is_module_syntax() sees `import` → module mode.
// v8::script_compiler::compile_module() then parses `{ type Something, ... }`
// and errors: "Unexpected identifier 'Something'".
//
// Expected (Rivers v0.54.1): 500 module compilation failed.
// This is also the exact error CB encountered across its handler suite.

import { type Something, foo } from './case-d-helpers';

function handler(ctx) {
    ctx.resdata = { case: "D", outcome: "pass" };
}
