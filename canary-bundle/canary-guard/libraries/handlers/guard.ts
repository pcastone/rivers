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
            subject: "canary-user-001",
            username: body.username,
            role: "tester",
            groups: ["canary-fleet"]
        };
    }

    throw new Error("invalid credentials");
}
