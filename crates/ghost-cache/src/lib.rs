//! ghost-cache: event-driven UIA mirror + in-memory locator cache.
// Ghost's engine is Windows-only today. Off Windows this crate compiles to
// nothing and its build script fails with a one-line explanation; see
// docs/cross-platform.md and docs/plans/2026-07-cross-platform-plan.md.
#![cfg(windows)]
// Pre-existing lint, tracked with the ghost-core cleanup pass.
#![allow(clippy::type_complexity)]

pub mod uia_mirror;
pub mod locator_cache;
pub mod error;

pub use error::CacheError;
pub use locator_cache::{LocatorCache, LocatorHitResult, LocatorCacheStats};
