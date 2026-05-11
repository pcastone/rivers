// OTLP smoke handler — minimal viable per-signal ingest.
//
// Counts the number of metric/log/trace records on the inbound payload,
// emits a partialSuccess if exactly one record is flagged (so the smoke
// can verify the framework wraps the response correctly), and otherwise
// returns nothing — framework emits the canonical `200 {}`.

interface Ctx {
    otel: {
        kind: 'metrics' | 'logs' | 'traces';
        payload: Record<string, unknown>;
        encoding: 'json' | 'protobuf';
    };
}

function countMetrics(payload: any): number {
    let n = 0;
    for (const rm of payload?.resourceMetrics ?? []) {
        for (const sm of rm?.scopeMetrics ?? []) {
            for (const _m of sm?.metrics ?? []) {
                n += 1;
            }
        }
    }
    return n;
}

function countLogs(payload: any): number {
    let n = 0;
    for (const rl of payload?.resourceLogs ?? []) {
        for (const sl of rl?.scopeLogs ?? []) {
            for (const _r of sl?.logRecords ?? []) {
                n += 1;
            }
        }
    }
    return n;
}

function countTraces(payload: any): number {
    let n = 0;
    for (const rs of payload?.resourceSpans ?? []) {
        for (const ss of rs?.scopeSpans ?? []) {
            for (const _s of ss?.spans ?? []) {
                n += 1;
            }
        }
    }
    return n;
}

// Probe convention: when the inbound `_probe_reject` marker is present
// at the payload root (added by the smoke script), the handler returns
// rejected=1 + errorMessage="probe-rejected" so the smoke can verify the
// framework's partialSuccess response wrapping. Otherwise the handler
// returns the count it saw so the smoke can confirm the body was parsed
// correctly through every transport (JSON, gzip, protobuf).

export function ingestMetrics(ctx: Ctx): { rejected?: number; errorMessage?: string; ingested?: number; encoding?: string } {
    const payload: any = ctx.otel.payload;
    if (payload?._probe_reject === true) {
        return { rejected: 1, errorMessage: 'probe-rejected' };
    }
    return { ingested: countMetrics(payload), encoding: ctx.otel.encoding };
}

export function ingestLogs(ctx: Ctx): { rejected?: number; errorMessage?: string; ingested?: number; encoding?: string } {
    const payload: any = ctx.otel.payload;
    if (payload?._probe_reject === true) {
        return { rejected: 1, errorMessage: 'probe-rejected' };
    }
    return { ingested: countLogs(payload), encoding: ctx.otel.encoding };
}

export function ingestTraces(ctx: Ctx): { rejected?: number; errorMessage?: string; ingested?: number; encoding?: string } {
    const payload: any = ctx.otel.payload;
    if (payload?._probe_reject === true) {
        return { rejected: 1, errorMessage: 'probe-rejected' };
    }
    return { ingested: countTraces(payload), encoding: ctx.otel.encoding };
}
