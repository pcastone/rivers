// Outbound HTTP handler — Rivers.http for external API calls
//
// Requires allow_outbound_http = true on the view:
//
//   [api.views.proxy.handler]
//   type                = "codecomponent"
//   language            = "javascript"
//   module              = "libraries/handlers/proxy.js"
//   entrypoint          = "fetchWeather"
//   allow_outbound_http = true

// GET request to external API
async function fetchWeather(ctx) {
    var city = ctx.request.query_params.city || "London";

    var resp = await Rivers.http.get("https://api.example.com/weather?city=" + encodeURIComponent(city));

    if (resp.status !== 200) {
        Rivers.log.error("weather API failed", { status: resp.status, city: city });
        throw new Error("weather service unavailable");
    }

    ctx.resdata = {
        city: city,
        weather: resp.body,
        fetched_at: new Date().toISOString()
    };
}

// POST to external API with body
async function createWebhook(ctx) {
    var body = ctx.request.body;

    var resp = await Rivers.http.post("https://hooks.example.com/notify", {
        event: body.event,
        payload: body.data,
        source: ctx.app_id,
        trace_id: ctx.trace_id
    });

    Rivers.log.info("webhook sent", { status: resp.status, event: body.event });

    ctx.resdata = {
        sent: true,
        status: resp.status,
        webhook_response: resp.body
    };
}

// Aggregate data from multiple external APIs
async function aggregateApis(ctx) {
    var [users, products, stats] = await Promise.all([
        Rivers.http.get("https://api.example.com/users?limit=5"),
        Rivers.http.get("https://api.example.com/products?limit=5"),
        Rivers.http.get("https://api.example.com/stats")
    ]);

    ctx.resdata = {
        users: users.body,
        products: products.body,
        stats: stats.body,
        aggregated_at: new Date().toISOString()
    };
}
