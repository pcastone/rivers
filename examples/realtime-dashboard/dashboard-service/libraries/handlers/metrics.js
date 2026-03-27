// Streaming handler for SSE metrics feed
// Uses the {chunk, done} protocol — Rivers calls this repeatedly
// State is passed between iterations via __args.state and __args.iteration
function streamMetrics(ctx) {
    var iteration = __args.iteration || 0;

    // Stop after 1000 iterations (or let stream_timeout_ms handle it)
    if (iteration >= 1000) {
        return { done: true };
    }

    // Generate a metric snapshot
    var metric = {
        host: "server-" + (iteration % 5 + 1),
        cpu_percent: Math.round(Math.random() * 100 * 10) / 10,
        mem_mb: Math.floor(Math.random() * 16384),
        disk_percent: Math.round(Math.random() * 100 * 10) / 10,
        request_count: Math.floor(Math.random() * 500),
        error_rate: Math.round(Math.random() * 5 * 100) / 100,
        timestamp: new Date().toISOString()
    };

    return {
        chunk: metric,
        done: false
    };
}

// REST endpoint to get current metric snapshot
function getSnapshot(ctx) {
    var data = ctx.data.list_metrics;
    ctx.resdata = data;
}
