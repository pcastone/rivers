// Case C — variable type annotation.
// Test at process_pool_tests.rs:288 only asserts contains("const x"); it does
// NOT verify ": number" is stripped. The stripper leaves variable annotations
// in place.
//
// Expected: 500 SyntaxError: Unexpected token ':'

function handler(ctx) {
    const answer: number = 42;
    ctx.resdata = { case: "C", outcome: "pass", answer: answer };
}
