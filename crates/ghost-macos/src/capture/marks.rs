//! Set-of-Marks overlay data. Pure data (no drawing needed here -- nothing on
//! this crate ever successfully captures a frame to draw marks onto), ported
//! for real so the type shape matches `ghost_core::capture::Mark`.

#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub label: u32,
    pub x: i32,
    pub y: i32,
}
