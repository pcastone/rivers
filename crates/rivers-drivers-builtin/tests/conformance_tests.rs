//! Driver conformance matrix — cross-driver tests for bug classes.
//!
//! Run with: `cargo test -p rivers-drivers-builtin --test conformance_tests`
//! Cluster tests: `RIVERS_TEST_CLUSTER=1 cargo test -p rivers-drivers-builtin --test conformance_tests`

mod conformance;

mod conformance_ddl_guard {
    include!("conformance/ddl_guard.rs");
}

mod conformance_crud_lifecycle {
    include!("conformance/crud_lifecycle.rs");
}

mod conformance_param_binding {
    include!("conformance/param_binding.rs");
}

mod conformance_h18_mysql_uint {
    include!("conformance/h18_mysql_uint.rs");
}

mod conformance_h4_mysql_pool_key {
    include!("conformance/h4_mysql_pool_key.rs");
}
