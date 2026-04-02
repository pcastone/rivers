use std::collections::HashMap;
use rivers_driver_sdk::{translate_params, ParamStyle, QueryValue};

#[test]
fn dollar_positional_rewrites_in_order() {
    let mut params = HashMap::new();
    params.insert("id".to_string(), QueryValue::Integer(42));
    params.insert("status".to_string(), QueryValue::String("active".into()));

    let stmt = "SELECT * FROM orders WHERE id = $id AND status = $status";
    let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::DollarPositional);

    assert_eq!(rewritten, "SELECT * FROM orders WHERE id = $1 AND status = $2");
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].0, "id");
    assert_eq!(ordered[1].0, "status");
}

#[test]
fn question_positional_rewrites() {
    let mut params = HashMap::new();
    params.insert("name".to_string(), QueryValue::String("alice".into()));
    params.insert("age".to_string(), QueryValue::Integer(30));

    let stmt = "INSERT INTO users (name, age) VALUES ($name, $age)";
    let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::QuestionPositional);

    assert_eq!(rewritten, "INSERT INTO users (name, age) VALUES (?, ?)");
    assert_eq!(ordered[0].0, "name");
    assert_eq!(ordered[1].0, "age");
}

#[test]
fn colon_named_rewrites() {
    let mut params = HashMap::new();
    params.insert("id".to_string(), QueryValue::Integer(1));

    let stmt = "SELECT * FROM users WHERE id = $id";
    let (rewritten, _) = translate_params(stmt, &params, ParamStyle::ColonNamed);

    assert_eq!(rewritten, "SELECT * FROM users WHERE id = :id");
}

#[test]
fn dollar_named_passthrough() {
    let mut params = HashMap::new();
    params.insert("id".to_string(), QueryValue::Integer(1));

    let stmt = "SELECT * FROM users WHERE id = $id";
    let (rewritten, _) = translate_params(stmt, &params, ParamStyle::DollarNamed);

    assert_eq!(rewritten, "SELECT * FROM users WHERE id = $id");
}

#[test]
fn none_passthrough() {
    let mut params = HashMap::new();
    params.insert("key".to_string(), QueryValue::String("mykey".into()));

    let stmt = "GET mykey";
    let (rewritten, _) = translate_params(stmt, &params, ParamStyle::None);

    assert_eq!(rewritten, "GET mykey");
}

#[test]
fn preserves_order_of_appearance() {
    let mut params = HashMap::new();
    params.insert("z_last".to_string(), QueryValue::Integer(1));
    params.insert("a_first".to_string(), QueryValue::Integer(2));

    // z_last appears first in query text, despite being alphabetically last
    let stmt = "SELECT * FROM t WHERE z = $z_last AND a = $a_first";
    let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::DollarPositional);

    assert_eq!(rewritten, "SELECT * FROM t WHERE z = $1 AND a = $2");
    // Order follows appearance in query, NOT alphabetical
    assert_eq!(ordered[0].0, "z_last");
    assert_eq!(ordered[1].0, "a_first");
}

#[test]
fn no_placeholders_in_query() {
    let params = HashMap::new();
    let stmt = "SELECT 1";
    let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::DollarPositional);

    assert_eq!(rewritten, "SELECT 1");
    assert_eq!(ordered.len(), 0);
}

#[test]
fn duplicate_placeholder_handled() {
    let mut params = HashMap::new();
    params.insert("id".to_string(), QueryValue::Integer(42));

    let stmt = "SELECT * FROM t WHERE id = $id OR parent_id = $id";
    let (rewritten, ordered) = translate_params(stmt, &params, ParamStyle::DollarPositional);

    assert_eq!(rewritten, "SELECT * FROM t WHERE id = $1 OR parent_id = $1");
    assert_eq!(ordered.len(), 1);
}
