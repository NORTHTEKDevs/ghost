//! Background browser automation for Ghost via the Chrome DevTools Protocol.
//!
//! The Win32 backends in `ghost-core` cannot drive a browser properly: Chromium is a
//! single HWND with no child controls, so window messages have nothing to target, and
//! its UI Automation tree is a flattened approximation of the DOM. Driving a browser
//! by moving the real cursor over it is what forces "don't touch the computer while
//! it runs".
//!
//! CDP removes the problem at the root. Input events are injected into a specific
//! renderer's event queue, addressed by tab. A tab does not need to be in front, its
//! window does not need focus, and the OS cursor never moves - so the user keeps
//! typing while any number of tabs are automated underneath.

pub mod browser;
pub mod cdp;
pub mod error;
pub mod keys;
pub mod launch;
pub mod tab;

pub use browser::Browser;
pub use error::{BrowserError, Result};
pub use launch::{find_named_browser, installed_browsers, LaunchMode, LaunchOptions};
pub use tab::{Tab, TabInfo};
