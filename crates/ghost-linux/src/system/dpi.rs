//! Display scaling.
//!
//! Windows needs an explicit per-monitor-v2 DPI-awareness call or every click
//! coordinate is wrong above 100% scaling. Linux has no process-wide equivalent:
//! X11 reports pixels directly, and on Wayland the compositor works in logical
//! pixels which is already the space AT-SPI reports extents in.
//!
//! The real Linux scaling hazard is different, and it is not solved by a call
//! like this: under fractional scaling a portal/PipeWire screenshot buffer can
//! be in *device* pixels while AT-SPI extents are in *logical* pixels. Capture
//! code must therefore scale explicitly rather than assume 1:1 -- see
//! `capture::scale_factor`.

/// Mirrors `ghost_core::system::dpi::ensure_process_dpi_aware`. No-op on Linux.
pub fn ensure_process_dpi_aware() {}

/// Best-effort logical-to-device scale factor.
///
/// `GDK_SCALE` / `QT_SCALE_FACTOR` are the only hints reliably visible to a
/// non-GUI process; when unset, 1.0 is correct for the overwhelming majority of
/// sessions. Returns 1.0 rather than guessing when the value is unparseable.
pub fn scale_factor() -> f64 {
    for var in ["GDK_SCALE", "QT_SCALE_FACTOR"] {
        if let Ok(v) = std::env::var(var) {
            if let Ok(f) = v.trim().parse::<f64>() {
                if f > 0.0 {
                    return f;
                }
            }
        }
    }
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpi_awareness_is_a_no_op() {
        ensure_process_dpi_aware();
    }

    #[test]
    fn scale_factor_is_positive() {
        assert!(scale_factor() > 0.0);
    }
}
