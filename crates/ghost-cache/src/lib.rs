//! ghost-cache: event-driven UIA mirror + in-memory locator cache.
pub mod uia_mirror;
pub mod locator_cache;
pub mod error;

pub use error::CacheError;
pub use locator_cache::{LocatorCache, LocatorHitResult, LocatorCacheStats};
