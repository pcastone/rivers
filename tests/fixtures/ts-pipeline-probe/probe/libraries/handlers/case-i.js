// Case I — ctx.dataview() with pre-fetched resource.
// The view declares `resources = ["probe_select"]`, so ctx.data.probe_select
// is populated before the handler runs. This proves the happy path that CB
// must migrate all 143 inline ctx.sql calls onto.
//
// Expected: 200 { "case": "I", "outcome": "pass", "rows": [{"answer": 1}] }

function handler(ctx) {
    var rows = ctx.data.probe_select;
    ctx.resdata = {
        case: "I",
        outcome: "pass",
        rows: rows,
        dynamic_call: ctx.dataview("probe_select", {})
    };
}
