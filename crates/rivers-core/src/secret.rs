//! `Secret<T>` — a zeroizing wrapper for sensitive values.
//!
//! Guarantees:
//! - `Debug` is always redacted (`"<redacted>"`), preventing accidental logging.
//! - `Clone` is banned by default; use `.clone_secret()` where `T: Clone` with
//!   explicit intent.
//! - `Drop` zeroizes the inner value so secret bytes don't linger in memory.
//! - `.expose(&self) -> &T` is the only way to access the inner value.

use zeroize::Zeroize;

/// A wrapper that holds a secret value `T`, zeroizes it on drop, and
/// never exposes it through `Debug`.
///
/// # Cloning
/// `Clone` is intentionally not derived. Implicit clones of secrets are a
/// common source of accidental secret duplication in memory. Use
/// `.clone_secret()` when you genuinely need a copy — the explicit call
/// makes the intent visible in code review.
pub struct Secret<T: Zeroize>(T);

impl<T: Zeroize> Secret<T> {
    /// Wrap a value as a secret.
    pub fn new(value: T) -> Self {
        Self(value)
    }

    /// Access the inner value.
    ///
    /// The name `expose` is intentional — it makes it visible at the call site
    /// that you are working with sensitive material.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T: Zeroize + Clone> Secret<T> {
    /// Explicitly clone the secret value.
    ///
    /// `Clone` is not derived on `Secret<T>` to prevent accidental duplication
    /// of secret material. This method exists for the rare cases where a copy
    /// is genuinely required; the call is visible and intentional.
    pub fn clone_secret(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Zeroize> Drop for Secret<T> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<T: Zeroize> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T: Zeroize> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_is_redacted() {
        let secret = Secret::new("my-password".to_string());
        assert_eq!(format!("{:?}", secret), "<redacted>");
    }

    #[test]
    fn debug_is_redacted_for_vec() {
        let secret = Secret::new(vec![1u8, 2, 3]);
        assert_eq!(format!("{:?}", secret), "<redacted>");
    }

    #[test]
    fn expose_returns_inner_value() {
        let secret = Secret::new("hello".to_string());
        assert_eq!(secret.expose(), "hello");
    }

    #[test]
    fn clone_secret_produces_independent_copy() {
        let s1 = Secret::new("value".to_string());
        let s2 = s1.clone_secret();
        assert_eq!(s1.expose(), s2.expose());
    }

    #[test]
    fn drop_zeroizes_inner_value() {
        // Verify that `Secret::drop` calls `Zeroize::zeroize` on the inner value.
        //
        // We use a custom type that records whether `zeroize()` was called,
        // stored in a shared `Cell<bool>` outside the `Secret` so we can observe
        // it after drop without accessing freed memory.
        use std::cell::Cell;
        use std::rc::Rc;

        #[derive(Clone)]
        struct TrackZeroize(Rc<Cell<bool>>);

        impl Zeroize for TrackZeroize {
            fn zeroize(&mut self) {
                self.0.set(true);
            }
        }

        let called = Rc::new(Cell::new(false));
        let tracker = TrackZeroize(Rc::clone(&called));

        // Wrap in Secret and then drop it.
        let secret = Secret::new(tracker);
        assert!(!called.get(), "zeroize must not be called before drop");
        drop(secret);
        assert!(called.get(), "Secret::drop must call Zeroize::zeroize on inner value");
    }
}
