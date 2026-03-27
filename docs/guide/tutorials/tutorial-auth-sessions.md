# Tutorial: Authentication & Sessions

**Rivers v0.50.1**

## Overview

Rivers provides built-in session-based authentication. You create a **guard view** (the login endpoint), and Rivers handles session creation, validation, and expiry. Protected views check the session automatically.

---

## Architecture

```
Client → POST /api/login (guard view)
       → Handler validates credentials
       → Handler returns claims { subject, username, groups }
       → Rivers creates session, returns session token

Client → GET /api/profile (auth = "session")
       → Rivers validates session token automatically
       → Handler receives ctx.session with the claims
```

---

## Step 1: Create the Login Endpoint (Guard View)

File: `app.toml`

```toml
[api.views.login]
path      = "auth/login"
method    = "POST"
view_type = "Rest"
auth      = "none"
guard     = true

[api.views.login.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/auth.js"
entrypoint = "login"
resources  = ["users_db"]
```

Key fields:
- `auth = "none"` — the login endpoint itself is public
- `guard = true` — marks this as the auth endpoint that creates sessions

---

## Step 2: Write the Guard Handler

File: `libraries/handlers/auth.js`

```javascript
function login(ctx) {
    var body = ctx.request.body;

    if (!body || !body.username || !body.password) {
        throw new Error("username and password are required");
    }

    // Look up user
    var user = ctx.dataview("get_user_by_username", {
        username: body.username
    });

    if (!user) {
        Rivers.log.warn("login failed — user not found", {
            username: body.username
        });
        throw new Error("invalid credentials");
    }

    // Verify password
    var valid = Rivers.crypto.verifyPassword(body.password, user.password_hash);

    if (!valid) {
        Rivers.log.warn("login failed — bad password", {
            username: body.username
        });
        throw new Error("invalid credentials");
    }

    Rivers.log.info("login successful", {
        user_id: user.id,
        username: user.username
    });

    // Return claims — Rivers creates the session automatically
    // These claims are available as ctx.session in protected views
    return {
        subject: user.id,
        username: user.username,
        email: user.email,
        groups: user.groups || ["user"]
    };
}
```

**Important:** In a guard handler, the **return value** (not `ctx.resdata`) becomes the session claims. Rivers wraps this in a session and returns the token to the client.

---

## Step 3: Create Protected Endpoints

```toml
# Protected — requires valid session
[api.views.profile]
path      = "auth/profile"
method    = "GET"
view_type = "Rest"
auth      = "session"

[api.views.profile.handler]
type       = "codecomponent"
language   = "javascript"
module     = "libraries/handlers/auth.js"
entrypoint = "getProfile"
resources  = ["users_db"]

# Public — no session required
[api.views.health]
path      = "health"
method    = "GET"
view_type = "Rest"
auth      = "none"
```

---

## Step 4: Access Session in Handlers

```javascript
function getProfile(ctx) {
    // ctx.session is populated by Rivers when auth = "session"
    var session = ctx.session;

    // session contains the claims returned by the guard handler
    var userId = session.subject;
    var username = session.username;
    var groups = session.groups;

    var user = ctx.dataview("get_user", { id: userId });

    ctx.resdata = {
        id: user.id,
        username: username,
        email: user.email,
        groups: groups
    };
}
```

---

## Step 5: Register Users (Password Hashing)

```javascript
function register(ctx) {
    var body = ctx.request.body;

    if (!body.username || !body.password || !body.email) {
        throw new Error("username, password, and email are required");
    }

    // Hash the password before storing
    var hash = Rivers.crypto.hashPassword(body.password);

    var user = ctx.dataview("create_user", {
        username: body.username,
        email: body.email,
        password_hash: hash,
        groups: ["user"]
    });

    Rivers.log.info("user registered", { user_id: user.id });
    ctx.resdata = { id: user.id, username: user.username };
}
```

---

## Testing

```bash
# Register a user
curl -X POST http://localhost:8080/auth/register \
  -H "Content-Type: application/json" \
  -d '{"username":"alice","password":"secret123","email":"alice@example.com"}'

# Login — returns session token
curl -X POST http://localhost:8080/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username":"alice","password":"secret123"}'

# Response: { "token": "eyJ...", "claims": { "subject": "...", ... } }

# Access protected endpoint with session token
curl http://localhost:8080/auth/profile \
  -H "Authorization: Bearer eyJ..."

# Without token — returns 401
curl http://localhost:8080/auth/profile
# {"code": 401, "message": "unauthorized"}
```

---

## Auth Configuration Reference

### View Fields

| Field | Values | Description |
|-------|--------|-------------|
| `auth` | `"none"` | No authentication check |
| `auth` | `"session"` | Requires valid session token |
| `guard` | `true` / `false` | Marks view as login endpoint |

### Session Claims (ctx.session)

| Field | Description |
|-------|-------------|
| `subject` | User identifier (typically user ID) |
| `username` | Username |
| `email` | User email |
| `groups` | Array of group/role names |

You can include any additional fields in the claims — they'll be available in `ctx.session`.

### Rivers.crypto API

| Function | Description |
|----------|-------------|
| `hashPassword(plain)` | Returns bcrypt hash |
| `verifyPassword(plain, hash)` | Returns boolean |
| `randomHex(bytes)` | Random hex string |
| `randomBase64url(bytes)` | URL-safe random token |
| `hmac(key, data)` | HMAC-SHA256 signature |
| `timingSafeEqual(a, b)` | Constant-time comparison |
