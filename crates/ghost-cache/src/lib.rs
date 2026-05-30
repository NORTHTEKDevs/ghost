//! ghost-cache: event-driven UIA mirror + SQLite-backed locator store.
pub mod uia_mirror;
pub mod locator_store;
pub mod locator_cache;
pub mod error;

pub use error::CacheError;
pub use locator_cache::{LocatorCache, LocatorHitResult, LocatorCacheStats};
