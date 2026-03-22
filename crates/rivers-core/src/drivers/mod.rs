//! Built-in database drivers — re-exported from `rivers-drivers-builtin`.
//!
//! When the `drivers` feature is enabled, this module re-exports all
//! built-in drivers from the `rivers-drivers-builtin` crate. When disabled,
//! this module is empty and no driver dependencies are linked.

pub use rivers_drivers_builtin::*;

use rivers_driver_sdk::DriverRegistrar;

/// Register all built-in drivers into the given factory.
///
/// Delegates to `rivers_drivers_builtin::register_builtin_drivers()`.
pub fn register_builtin_drivers(factory: &mut dyn DriverRegistrar) {
    rivers_drivers_builtin::register_builtin_drivers(factory);
}
