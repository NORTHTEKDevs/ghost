pub mod error;
pub mod locator;
pub mod element;
pub mod session;
pub mod background;

pub use session::{GhostSession, Region};
pub use locator::By;
pub use element::{ActionRoute, GhostElement};
pub use background::WindowTarget;
pub use error::GhostError;

/// Join the calling thread to the multithreaded COM apartment. Idempotent.
pub fn init_com_for_thread() -> bool {
    ghost_core::uia::init_com().is_ok()
}
pub use ghost_core::uia::{ElementDescriptor, WindowInfo};
pub use ghost_core::system::{DesktopDelta, DesktopSnapshot};
pub use ghost_core::focus::FocusPolicy;
