// Case F — multi-module import (relative).
// execution.rs:instantiate_module resolve callback returns None for every
// specifier. Documented limitation ("V1 -- no multi-module").
//
// Expected: 500 module instantiation failed.

import { helper } from './case-f-helpers';

function handler(ctx) {
    ctx.resdata = { case: "F", outcome: "pass", helped: helper() };
}
