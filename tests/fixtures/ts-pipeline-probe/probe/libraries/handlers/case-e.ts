// Case E — generic type parameter.
// Docstring claims `<T>` → removed. strip_type_annotations() has no such code.
//
// Expected: 500 SyntaxError on `function identity<T>`

function identity<T>(x: T): T { return x; }

function handler(ctx) {
    ctx.resdata = { case: "E", outcome: "pass", echoed: identity("ok") };
}
