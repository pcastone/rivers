// Broker-publish atomic tests — BR-2026-04-23 (BR4).
// Exercises the new `ctx.datasource("<broker>").publish(...)` surface
// introduced by the MessageBrokerDriver V8 bridge.
//
// Cross-app imports forbidden; reuse the inline TestResult pattern from
// kafka-consumer.ts rather than importing test-harness.ts.

function TestResult(test_id: string, profile: string, spec_ref: string) {
    (this as any).test_id = test_id;
    (this as any).profile = profile;
    (this as any).spec_ref = spec_ref;
    (this as any).assertions = [];
    (this as any).error = null;
    (this as any).start = Date.now();
}
(TestResult as any).prototype.assert = function (id: string, passed: boolean, detail?: string) {
    (this as any).assertions.push({ id, passed, detail: detail || undefined });
};
(TestResult as any).prototype.finish = function () {
    return {
        test_id: (this as any).test_id,
        profile: (this as any).profile,
        spec_ref: (this as any).spec_ref,
        passed: (this as any).assertions.every((a: any) => a.passed),
        assertions: (this as any).assertions,
        duration_ms: Date.now() - (this as any).start,
        error: (this as any).error,
    };
};
(TestResult as any).prototype.fail = function (err: string) {
    (this as any).error = err;
    return {
        test_id: (this as any).test_id,
        profile: (this as any).profile,
        spec_ref: (this as any).spec_ref,
        passed: false,
        assertions: (this as any).assertions,
        duration_ms: Date.now() - (this as any).start,
        error: err,
    };
};

// ── STREAM-KAFKA-PUBLISH-RECEIPT (BR4.1) ───────────────────────

export function kafkaPublishReceipt(ctx: any): void {
    const t = new (TestResult as any)(
        "STREAM-KAFKA-PUBLISH-RECEIPT",
        "STREAM",
        "bugs/bugreport_2026-04-23.md BR4.1"
    );
    try {
        const broker = ctx.datasource("canary-kafka");
        t.assert("datasource_returns_object",
            broker !== null && broker !== undefined && typeof broker === "object");
        t.assert("publish_is_function", typeof broker.publish === "function",
            "type=" + typeof broker.publish);

        const receipt = broker.publish({
            destination: "canary.test.publish-receipt",
            payload: "hello from br4",
            headers: { "source": "canary" }
        });
        t.assert("receipt_returned", receipt !== null && receipt !== undefined,
            "type=" + typeof receipt);
        if (receipt) {
            // Kafka populates id (offset string) + metadata (partition).
            // Both Option<String> on the Rust side — may be null on some brokers.
            t.assert("receipt_has_id_field", "id" in receipt,
                "keys=" + Object.keys(receipt).join(","));
            t.assert("receipt_has_metadata_field", "metadata" in receipt);
        }
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}

// ── STREAM-KAFKA-PUBLISH-UNKNOWN-DATASOURCE (BR4.3) ────────────

export function kafkaPublishUnknownDatasource(ctx: any): void {
    const t = new (TestResult as any)(
        "STREAM-KAFKA-PUBLISH-UNKNOWN-DATASOURCE",
        "STREAM",
        "bugs/bugreport_2026-04-23.md BR4.3"
    );
    try {
        let threw = false;
        let errMsg = "";
        try {
            const broker = ctx.datasource("this-broker-does-not-exist");
            // If the fallback builder is returned (pseudo-DV), publish() isn't a method.
            if (broker && typeof broker.publish === "function") {
                broker.publish({ destination: "x", payload: "x" });
            } else {
                // Expected path: no broker proxy registered → fallback has no publish.
                throw new Error("no publish method on unknown datasource");
            }
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }
        t.assert("rejected", threw, threw ? ("threw: " + errMsg) : "did not throw");
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}

// ── STREAM-KAFKA-PUBLISH-MISSING-DESTINATION (BR4.4) ───────────

export function kafkaPublishMissingDestination(ctx: any): void {
    const t = new (TestResult as any)(
        "STREAM-KAFKA-PUBLISH-MISSING-DESTINATION",
        "STREAM",
        "bugs/bugreport_2026-04-23.md BR4.4"
    );
    try {
        const broker = ctx.datasource("canary-kafka");
        let threw = false;
        let errMsg = "";
        try {
            broker.publish({ payload: "no destination here" });
        } catch (e) {
            threw = true;
            errMsg = String(e);
        }
        t.assert("rejected", threw);
        if (threw) {
            t.assert("error_names_destination",
                errMsg.indexOf("destination") !== -1,
                "err=" + errMsg);
        }
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}

// ── STREAM-KAFKA-PUBLISH-THEN-CONSUME (BR4.2) ──────────────────
// Publishes a unique message and returns the id so a follow-up REST
// probe (STREAM-KAFKA-VERIFY in kafka-consumer.ts) can confirm the
// MessageConsumer view received it.

export function kafkaPublishThenConsume(ctx: any): void {
    const t = new (TestResult as any)(
        "STREAM-KAFKA-PUBLISH-THEN-CONSUME",
        "STREAM",
        "bugs/bugreport_2026-04-23.md BR4.2"
    );
    try {
        const broker = ctx.datasource("canary-kafka");
        const marker = "br4-marker-" + ctx.trace_id;
        const receipt = broker.publish({
            destination: "canary.kafka.test",
            payload: marker,
            headers: { "trace_id": ctx.trace_id }
        });
        t.assert("published_no_throw", true);
        t.assert("receipt_returned", receipt !== null && receipt !== undefined);
        // Stash the marker so STREAM-KAFKA-VERIFY can check the consumer
        // saw a message with this payload within the last 60s.
        ctx.store.set("canary:kafka:publish-marker", marker, 60000);
    } catch (e) {
        ctx.resdata = t.fail(String(e));
        return;
    }
    ctx.resdata = t.finish();
}
