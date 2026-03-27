// Async handler — parallel DataView calls with Promise.all
//
// Config:
//   [api.views.dashboard.handler]
//   type       = "codecomponent"
//   language   = "javascript"
//   module     = "libraries/handlers/dashboard.js"
//   entrypoint = "getDashboard"
//   resources  = ["users_db", "orders_db", "metrics"]

async function getDashboard(ctx) {
    // Run multiple DataView queries in parallel
    var [users, orders, metrics] = await Promise.all([
        Promise.resolve(ctx.dataview("recent_users", { limit: 5 })),
        Promise.resolve(ctx.dataview("recent_orders", { limit: 10 })),
        Promise.resolve(ctx.dataview("system_metrics"))
    ]);

    ctx.resdata = {
        recent_users: users,
        recent_orders: orders,
        system_metrics: metrics,
        generated_at: new Date().toISOString()
    };
}

// Async with error handling
async function getOrderSummary(ctx) {
    var orderId = ctx.request.path_params.id;

    // Fetch order and related data in parallel
    var [order, items, customer] = await Promise.all([
        Promise.resolve(ctx.dataview("get_order", { id: orderId })),
        Promise.resolve(ctx.dataview("get_order_items", { order_id: orderId })),
        Promise.resolve(ctx.dataview("get_customer_by_order", { order_id: orderId }))
    ]);

    if (!order) {
        throw new Error("order not found");
    }

    ctx.resdata = {
        order: order,
        items: items || [],
        customer: customer
    };
}
