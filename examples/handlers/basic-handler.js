// Basic handler — read request, return data
//
// Config:
//   [api.views.my_view.handler]
//   type       = "codecomponent"
//   language   = "javascript"
//   module     = "libraries/handlers/basic.js"
//   entrypoint = "handler"
//   resources  = ["my_datasource"]

function handler(ctx) {
    // Read request info
    var method = ctx.request.method;       // "GET", "POST", etc.
    var path   = ctx.request.path;         // "/api/items/123"
    var query  = ctx.request.query_params; // { limit: "10", offset: "0" }
    var headers = ctx.request.headers;     // { "content-type": "application/json" }
    var body   = ctx.request.body;         // parsed JSON body (POST/PUT)
    var params = ctx.request.path_params;  // { id: "123" }

    // Access pre-fetched DataView results (from DataViews listed in handler.resources)
    var items = ctx.data.list_items;

    // Environment info
    var traceId = ctx.trace_id;
    var appId   = ctx.app_id;
    var env     = ctx.env;

    // Log with trace_id auto-included
    Rivers.log.info("handling request", { method: method, path: path });

    // Set response — this becomes the HTTP response body
    ctx.resdata = {
        items: items,
        request_method: method,
        trace_id: traceId
    };
}
