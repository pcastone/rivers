// Streaming handler — {chunk, done} protocol for streaming REST views
//
// Config:
//   [api.views.export]
//   path              = "/api/export"
//   method            = "POST"
//   view_type         = "Rest"
//   streaming         = true
//   streaming_format  = "ndjson"     # or "sse"
//   stream_timeout_ms = 60000
//
//   [api.views.export.handler]
//   type       = "codecomponent"
//   language   = "javascript"
//   module     = "libraries/handlers/streaming.js"
//   entrypoint = "exportData"
//   resources  = ["my_datasource"]

// Rivers calls this function repeatedly.
// __args.iteration = call count (0, 1, 2, ...)
// __args.state     = previous return value (for passing state between iterations)
function exportData(ctx) {
    var iteration = __args.iteration || 0;
    var totalRows = 50;

    // Signal completion
    if (iteration >= totalRows) {
        Rivers.log.info("export complete", { rows: totalRows });
        return { done: true };
    }

    // Generate a row of data
    var row = {
        row_number: iteration + 1,
        id: Rivers.crypto.randomHex(8),
        value: "row-" + (iteration + 1),
        exported_at: new Date().toISOString()
    };

    // Return chunk — Rivers sends this to the client as one ndjson line
    return {
        chunk: row,
        done: false
    };
}

// Streaming with state — pass data between iterations
function exportWithState(ctx) {
    var iteration = __args.iteration || 0;
    var state = __args.state || { cursor: 0, total_exported: 0 };

    if (state.cursor >= 100) {
        return { done: true };
    }

    // Fetch a batch from a DataView
    var batch = ctx.dataview("get_batch", { offset: state.cursor, limit: 10 });

    if (!batch || batch.length === 0) {
        return { done: true };
    }

    // Update state for next iteration
    return {
        chunk: { batch: batch, offset: state.cursor },
        done: false,
        // This becomes __args.state on next call
        state: {
            cursor: state.cursor + batch.length,
            total_exported: state.total_exported + batch.length
        }
    };
}
