//! Static driver → `OperationDescriptor` slice lookup for direct-dispatch drivers.
//!
//! Used by the typed-proxy codegen to enumerate a driver's operation surface
//! at V8 context-setup time. Drivers that opt into direct dispatch expose a
//! `FILESYSTEM_OPERATIONS`-style `pub static`; this module maps driver names
//! to those slices.

use rivers_runtime::rivers_core::drivers::filesystem::FILESYSTEM_OPERATIONS;
use rivers_runtime::rivers_driver_sdk::OperationDescriptor;

/// Return the operation catalog for a direct-dispatch driver, if one is registered.
pub(super) fn catalog_for(driver: &str) -> Option<&'static [OperationDescriptor]> {
    match driver {
        "filesystem" => Some(FILESYSTEM_OPERATIONS),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_catalog_has_eleven_ops() {
        let cat = catalog_for("filesystem").expect("filesystem registered");
        assert_eq!(cat.len(), 11);
    }

    #[test]
    fn unknown_driver_returns_none() {
        assert!(catalog_for("postgres").is_none());
        assert!(catalog_for("").is_none());
    }

    #[test]
    fn filesystem_catalog_contains_expected_names() {
        let cat = catalog_for("filesystem").unwrap();
        let names: Vec<&str> = cat.iter().map(|op| op.name).collect();
        for expected in [
            "readFile",
            "readDir",
            "stat",
            "exists",
            "find",
            "grep",
            "writeFile",
            "mkdir",
            "delete",
            "rename",
            "copy",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }
}
