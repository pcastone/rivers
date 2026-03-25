// Create a new todo item
// View config: method = "POST", handler.entrypoint = "createTodo"
function createTodo(ctx) {
    var body = ctx.request.body;

    if (!body || !body.title) {
        throw new Error("title is required");
    }

    var todo = {
        id: Rivers.crypto.randomHex(16),
        title: body.title,
        completed: false,
        priority: body.priority || 0,
        created_at: new Date().toISOString()
    };

    // Store in app KV (TTL 24 hours)
    ctx.store.set("todo:" + todo.id, todo, 86400000);

    Rivers.log.info("todo created", { id: todo.id, title: todo.title });
    ctx.resdata = todo;
}

// Update an existing todo item
// View config: method = "PUT", handler.entrypoint = "updateTodo"
function updateTodo(ctx) {
    var id = ctx.request.path_params.id;
    var body = ctx.request.body;

    var existing = ctx.store.get("todo:" + id);
    if (!existing) {
        throw new Error("todo not found");
    }

    if (body.title !== undefined) existing.title = body.title;
    if (body.completed !== undefined) existing.completed = body.completed;
    if (body.priority !== undefined) existing.priority = body.priority;

    ctx.store.set("todo:" + id, existing, 86400000);

    Rivers.log.info("todo updated", { id: id });
    ctx.resdata = existing;
}

// Delete a todo item
// View config: method = "DELETE", handler.entrypoint = "deleteTodo"
function deleteTodo(ctx) {
    var id = ctx.request.path_params.id;

    var existing = ctx.store.get("todo:" + id);
    if (!existing) {
        throw new Error("todo not found");
    }

    ctx.store.del("todo:" + id);

    Rivers.log.info("todo deleted", { id: id });
    ctx.resdata = { deleted: true, id: id };
}
