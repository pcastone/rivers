// KV store handler — ctx.store.set/get/del with TTL
//
// ctx.store provides application-scoped key-value storage
// backed by the StorageEngine (SQLite or Redis).
//
// Reserved prefixes (blocked): session:, csrf:, poll:, cache:, rivers:

// Rate limiter using KV store
function rateLimitedAction(ctx) {
    var ip = ctx.request.headers["x-forwarded-for"] || "unknown";
    var key = "ratelimit:" + ip;

    var counter = ctx.store.get(key);

    if (counter && counter.count >= 10) {
        throw new Error("rate limit exceeded — try again later");
    }

    if (!counter) {
        // First request — TTL 60 seconds (60000 ms)
        ctx.store.set(key, { count: 1 }, 60000);
    } else {
        ctx.store.set(key, { count: counter.count + 1 }, 60000);
    }

    // Do the actual work
    var result = ctx.dataview("process_action", { data: ctx.request.body });
    ctx.resdata = result;
}

// Shopping cart using KV store
function addToCart(ctx) {
    var body = ctx.request.body;
    var cartId = ctx.request.path_params.cart_id;
    var key = "cart:" + cartId;

    // Get existing cart or create new one (TTL 24 hours)
    var cart = ctx.store.get(key) || { items: [], updated_at: null };

    cart.items.push({
        product_id: body.product_id,
        quantity: body.quantity || 1,
        added_at: new Date().toISOString()
    });
    cart.updated_at = new Date().toISOString();

    ctx.store.set(key, cart, 86400000);

    Rivers.log.info("cart updated", { cart_id: cartId, item_count: cart.items.length });
    ctx.resdata = cart;
}

function getCart(ctx) {
    var cartId = ctx.request.path_params.cart_id;
    var cart = ctx.store.get("cart:" + cartId);

    if (!cart) {
        ctx.resdata = { items: [], updated_at: null };
        return;
    }

    ctx.resdata = cart;
}

function clearCart(ctx) {
    var cartId = ctx.request.path_params.cart_id;
    ctx.store.del("cart:" + cartId);
    ctx.resdata = { cleared: true };
}
