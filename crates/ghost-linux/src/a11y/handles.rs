//! Stable integer handles for AT-SPI objects.
//!
//! Ghost's shared layers pass windows and controls around as `isize` handles
//! (on Windows, an `HWND`). AT-SPI has no such handle: an object is addressed by
//! a `(unique D-Bus name, object path)` pair. This module interns those pairs
//! into stable `isize` values so the shared code works unchanged.
//!
//! Handles start at 1, so `0` means "none" -- matching the `HWND(0)` convention
//! the Windows engine already uses.
//!
//! Interning is monotonic and never reuses a slot. An address whose application
//! has exited resolves to an address that no longer answers on the bus, which
//! surfaces as [`crate::error::CoreError::WindowGone`] rather than silently
//! targeting a different, newer object. Reusing handles would risk exactly that
//! confusion.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// The address of an AT-SPI object: which application, and which object in it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ObjAddr {
    pub bus: String,
    pub path: String,
}

impl ObjAddr {
    pub fn new(bus: impl Into<String>, path: impl Into<String>) -> Self {
        Self { bus: bus.into(), path: path.into() }
    }
}

#[derive(Default)]
struct Registry {
    list: Vec<ObjAddr>,
    index: HashMap<ObjAddr, isize>,
}

fn registry() -> &'static Mutex<Registry> {
    static REG: OnceLock<Mutex<Registry>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(Registry::default()))
}

/// Intern an address, returning a stable handle. Idempotent: the same address
/// always yields the same handle.
pub fn intern(addr: &ObjAddr) -> isize {
    let mut reg = match registry().lock() {
        Ok(g) => g,
        // A poisoned registry must not take down the server; recover the guard.
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(&h) = reg.index.get(addr) {
        return h;
    }
    reg.list.push(addr.clone());
    let handle = reg.list.len() as isize; // 1-based; 0 stays reserved for "none"
    reg.index.insert(addr.clone(), handle);
    handle
}

/// Resolve a handle back to its address. `None` for `0` or an unknown handle.
pub fn resolve(handle: isize) -> Option<ObjAddr> {
    if handle <= 0 {
        return None;
    }
    let reg = match registry().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    reg.list.get((handle - 1) as usize).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_is_idempotent() {
        let a = ObjAddr::new(":1.42", "/org/a11y/atspi/accessible/1");
        let h1 = intern(&a);
        let h2 = intern(&a);
        assert_eq!(h1, h2);
    }

    #[test]
    fn distinct_addresses_get_distinct_handles() {
        let a = ObjAddr::new(":1.42", "/org/a11y/atspi/accessible/7");
        let b = ObjAddr::new(":1.42", "/org/a11y/atspi/accessible/8");
        let c = ObjAddr::new(":1.43", "/org/a11y/atspi/accessible/7");
        assert_ne!(intern(&a), intern(&b));
        assert_ne!(intern(&a), intern(&c));
    }

    #[test]
    fn round_trips_through_resolve() {
        let a = ObjAddr::new(":1.99", "/org/a11y/atspi/accessible/root");
        let h = intern(&a);
        assert_eq!(resolve(h).as_ref(), Some(&a));
    }

    #[test]
    fn zero_and_negative_are_never_valid() {
        // 0 is the "no window" sentinel the shared layers rely on.
        assert!(resolve(0).is_none());
        assert!(resolve(-1).is_none());
    }

    #[test]
    fn unknown_handle_resolves_to_none_rather_than_panicking() {
        assert!(resolve(isize::MAX).is_none());
    }
}
