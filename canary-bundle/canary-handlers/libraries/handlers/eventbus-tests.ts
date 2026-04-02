// EventBus test handler for RUNTIME profile.

function TestResult(test_id, profile, spec_ref) {
    this.test_id = test_id; this.profile = profile; this.spec_ref = spec_ref;
    this.assertions = []; this.error = null; this.start = Date.now();
}
TestResult.prototype.assert = function(id, passed, detail) {
    this.assertions.push({ id: id, passed: passed, detail: detail || undefined });
};
TestResult.prototype.finish = function() {
    return { test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: this.assertions.every(function(a) { return a.passed; }),
        assertions: this.assertions, duration_ms: Date.now() - this.start, error: this.error };
};
TestResult.prototype.fail = function(err) {
    this.error = err;
    return { test_id: this.test_id, profile: this.profile, spec_ref: this.spec_ref,
        passed: false, assertions: this.assertions, duration_ms: Date.now() - this.start, error: err };
};

// RT-EVENTBUS-PUBLISH — publish event via eventbus datasource
function eventbusPublish(ctx) {
    var t = new TestResult("RT-EVENTBUS-PUBLISH", "RUNTIME", "rivers-storage-engine-spec.md section 12");
    try {
        // EventBus is available as a datasource — publish an event
        var result = ctx.dataview("eventbus_publish", {
            topic: "canary.test",
            payload: JSON.stringify({ test: true, timestamp: new Date().toISOString() })
        });

        t.assert("publish_executed", true, "eventbus publish dispatched");
    } catch (e) {
        // EventBus may not be configured as a DataView — document the gap
        t.assert("eventbus_available", false, "error: " + String(e));
    }
    ctx.resdata = t.finish();
}
