pub mod error;
pub mod focus;
pub mod input;
pub mod uia;
pub mod capture;
pub mod desktop;
pub mod process;
pub mod system;

pub use desktop::{DesktopSession, DesktopWindow};
pub use focus::{policy as focus_policy, set_policy as set_focus_policy, FocusPolicy};
