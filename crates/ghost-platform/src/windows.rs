//! Windows backend — the full, verified Ghost engine.
//!
//! The actual implementation of every capability lives in `ghost-core` /
//! `ghost-session` (Win32 UI Automation for discovery, SendInput + posted window
//! messages for input incl. background dispatch, DXGI/GDI for capture) and is
//! exposed to agents through `ghost-mcp`. This backend simply declares that
//! Windows fulfils the whole contract, so `ghost-platform` can report the honest
//! three-version status without duplicating the engine.

use crate::{all_features, capabilities_for, Backend, Capabilities, Platform};

pub struct WindowsBackend;

impl Backend for WindowsBackend {
    fn platform(&self) -> Platform {
        Platform::Windows
    }
    fn capabilities(&self) -> Capabilities {
        // Full + functional; see the engine in ghost-core/ghost-session.
        let caps = capabilities_for(Platform::Windows);
        debug_assert!(caps.functional && caps.supported.len() == all_features().len());
        caps
    }
}
