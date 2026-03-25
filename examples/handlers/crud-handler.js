// CRUD handler — create, read, update, delete using ctx.dataview()
//
// Config: 4 views pointing to different entrypoints in this file
//   POST   /api/users → createUser
//   GET    /api/users/{id} → getUser
//   PUT    /api/users/{id} → updateUser
//   DELETE /api/users/{id} → deleteUser

function createUser(ctx) {
    var body = ctx.request.body;

    if (!body || !body.name || !body.email) {
        throw new Error("name and email are required");
    }

    // Call a DataView dynamically
    var result = ctx.dataview("create_user", {
        name: body.name,
        email: body.email,
        role: body.role || "member"
    });

    // Invalidation of list_users cache happens automatically
    // if create_user DataView has invalidates = ["list_users"]

    Rivers.log.info("user created", { email: body.email });
    ctx.resdata = result;
}

function getUser(ctx) {
    var id = ctx.request.path_params.id;

    var user = ctx.dataview("get_user", { id: id });

    if (!user) {
        throw new Error("user not found");
    }

    ctx.resdata = user;
}

function updateUser(ctx) {
    var id = ctx.request.path_params.id;
    var body = ctx.request.body;

    // Verify user exists first
    var existing = ctx.dataview("get_user", { id: id });
    if (!existing) {
        throw new Error("user not found");
    }

    var result = ctx.dataview("update_user", {
        id: id,
        name: body.name || existing.name,
        email: body.email || existing.email,
        role: body.role || existing.role
    });

    Rivers.log.info("user updated", { id: id });
    ctx.resdata = result;
}

function deleteUser(ctx) {
    var id = ctx.request.path_params.id;

    var existing = ctx.dataview("get_user", { id: id });
    if (!existing) {
        throw new Error("user not found");
    }

    ctx.dataview("delete_user", { id: id });

    Rivers.log.info("user deleted", { id: id });
    ctx.resdata = { deleted: true, id: id };
}
