// AUTH-GUARD-LOGIN + AUTH-GUARD-CLAIMS
// Guard view handler — validates credentials, returns IdentityClaims.
// Per auth-session-spec section 3: guard handler returns claims, framework creates session.

function login(ctx) {
    var body = ctx.request.body;

    if (!body || !body.username || !body.password) {
        throw new Error("username and password required");
    }

    // Canary accepts fixed test credentials
    if (body.username === "canary" && body.password === "canary-test") {
        // Return IdentityClaims — framework creates session from these
        return {
            sub: "canary-user-001",
            role: "tester",
            email: "canary@test.local",
            groups: ["canary-fleet"]
        };
    }

    throw new Error("invalid credentials");
}
