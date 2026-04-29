use rivers_driver_sdk::{is_ddl_statement, check_admin_guard, Query};

// ── is_ddl_statement ────────────────────────────────────────────

#[test]
fn ddl_create_table() {
    assert!(is_ddl_statement("CREATE TABLE users (id INT)"));
}

#[test]
fn ddl_alter_table() {
    assert!(is_ddl_statement("ALTER TABLE users ADD COLUMN email TEXT"));
}

#[test]
fn ddl_drop_table() {
    assert!(is_ddl_statement("DROP TABLE users CASCADE"));
}

#[test]
fn ddl_truncate() {
    assert!(is_ddl_statement("TRUNCATE TABLE users"));
}

#[test]
fn ddl_case_insensitive() {
    assert!(is_ddl_statement("create table foo (id int)"));
    assert!(is_ddl_statement("Drop Table bar"));
    assert!(is_ddl_statement("ALTER table baz ADD x INT"));
}

#[test]
fn ddl_leading_whitespace() {
    assert!(is_ddl_statement("  CREATE TABLE users (id INT)"));
    assert!(is_ddl_statement("\n\tDROP TABLE foo"));
}

#[test]
fn not_ddl_select() {
    assert!(!is_ddl_statement("SELECT * FROM users"));
}

#[test]
fn not_ddl_insert() {
    assert!(!is_ddl_statement("INSERT INTO users (name) VALUES ('alice')"));
}

#[test]
fn not_ddl_update() {
    assert!(!is_ddl_statement("UPDATE users SET name = 'bob' WHERE id = 1"));
}

#[test]
fn not_ddl_delete() {
    assert!(!is_ddl_statement("DELETE FROM users WHERE id = 1"));
}

#[test]
fn not_ddl_empty() {
    assert!(!is_ddl_statement(""));
    assert!(!is_ddl_statement("   "));
}

#[test]
fn not_ddl_create_without_space() {
    // "CREATE" alone or "CREATEX" should not match
    assert!(!is_ddl_statement("CREATEX foo"));
}

// ── check_admin_guard ───────────────────────────────────────────

#[test]
fn guard_blocks_ddl_statement() {
    let query = Query::new("users", "DROP TABLE users");
    let result = check_admin_guard(&query, &[]);
    assert!(result.is_some());
    assert!(result.unwrap().contains("DDL statement rejected"));
}

#[test]
fn guard_blocks_admin_operation() {
    let mut query = Query::new("cache", "");
    query.operation = "flushdb".to_string();
    let result = check_admin_guard(&query, &["flushdb", "flushall"]);
    assert!(result.is_some());
    assert!(result.unwrap().contains("admin operation 'flushdb' rejected"));
}

#[test]
fn guard_allows_normal_select() {
    let query = Query::new("users", "SELECT * FROM users");
    let result = check_admin_guard(&query, &["flushdb"]);
    assert!(result.is_none());
}

#[test]
fn guard_allows_normal_insert() {
    let query = Query::new("users", "INSERT INTO users (name) VALUES ('alice')");
    let result = check_admin_guard(&query, &[]);
    assert!(result.is_none());
}

#[test]
fn guard_allows_non_admin_operation() {
    let mut query = Query::new("cache", "");
    query.operation = "get".to_string();
    let result = check_admin_guard(&query, &["flushdb", "flushall"]);
    assert!(result.is_none());
}

#[test]
fn guard_ddl_takes_precedence_over_operation() {
    // Even if operation is "select", DDL statement text is caught
    let mut query = Query::new("users", "DROP TABLE users");
    query.operation = "select".to_string();
    let result = check_admin_guard(&query, &[]);
    assert!(result.is_some());
}

#[test]
fn guard_sanitizes_statement_not_echoed_in_message() {
    // RW1.1.b: error messages must NOT echo raw statement content —
    // a long statement containing credential material must not leak
    // into the user-facing error.
    let long_stmt = "DROP TABLE very_long_table_name_that_exceeds_forty_characters_easily CASCADE";
    let query = Query::new("db", long_stmt);
    let result = check_admin_guard(&query, &[]).unwrap();
    // Raw statement text must not appear in the error.
    assert!(
        !result.contains("CASCADE"),
        "error must not echo raw statement: {result}"
    );
    assert!(
        !result.contains("very_long_table_name"),
        "error must not echo raw statement: {result}"
    );
    // The error must still communicate that DDL was rejected.
    assert!(
        result.contains("DDL") || result.contains("rejected"),
        "error must indicate rejection: {result}"
    );
}
