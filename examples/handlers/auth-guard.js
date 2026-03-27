// Auth guard handler — login endpoint that creates sessions
//
// Config:
//   [api.views.login]
//   path      = "/api/login"
//   method    = "POST"
//   view_type = "Rest"
//   auth      = "none"
//   guard     = true
//
//   [api.views.login.handler]
//   type       = "codecomponent"
//   language   = "javascript"
//   module     = "libraries/handlers/auth.js"
//   entrypoint = "login"
//   resources  = ["users_db"]

// Guard handler — return value becomes session claims
function login(ctx) {
    var body = ctx.request.body;

    if (!body || !body.username || !body.password) {
        throw new Error("username and password are required");
    }

    // Look up user by username
    var user = ctx.dataview("get_user_by_username", { username: body.username });

    if (!user) {
        Rivers.log.warn("login failed — user not found", { username: body.username });
        throw new Error("invalid credentials");
    }

    // Verify password using Rivers crypto (bcrypt-based)
    var valid = Rivers.crypto.verifyPassword(body.password, user.password_hash);

    if (!valid) {
        Rivers.log.warn("login failed — bad password", { username: body.username });
        throw new Error("invalid credentials");
    }

    Rivers.log.info("login successful", { username: body.username, user_id: user.id });

    // Return claims — the framework creates the session automatically
    // These claims are available as ctx.session in session-protected views
    return {
        subject: user.id,
        username: user.username,
        email: user.email,
        groups: user.groups || ["user"]
    };
}

// Protected handler example — requires auth = "session" on the view
//
// Config:
//   [api.views.profile]
//   path      = "/api/profile"
//   method    = "GET"
//   view_type = "Rest"
//   auth      = "session"
function getProfile(ctx) {
    // ctx.session is populated automatically by Rivers when auth = "session"
    var session = ctx.session;

    var user = ctx.dataview("get_user", { id: session.subject });

    ctx.resdata = {
        id: user.id,
        username: user.username,
        email: user.email,
        groups: session.groups
    };
}
