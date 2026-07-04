//! Set-of-Marks overlay: draw numbered badges on a screenshot so a vision model
//! can pick a UI element by NUMBER instead of guessing raw pixel coordinates
//! (which vision models are unreliable at). Each badge is a colored box with a
//! white number, placed at an element's top-left. The caller maps the model's
//! chosen number back to the element's real rect.
//!
//! No font/imageproc dependency — digits are drawn from a hardcoded 5x7 bitmap.

use image::{Rgba, RgbaImage};

/// A mark to draw: 1-based label and its top-left position in image pixels.
#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub label: u32,
    pub x: i32,
    pub y: i32,
}

/// 5x7 bitmap for digits 0-9. Each of the 7 rows uses the low 5 bits (MSB = left).
const DIGITS: [[u8; 7]; 10] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // 0
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 1
    [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111], // 2
    [0b11111, 0b00010, 0b00100, 0b00110, 0b00001, 0b10001, 0b01110], // 3
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // 4
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // 5
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // 6
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // 7
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // 8
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // 9
];

const SCALE: i32 = 2; // each font pixel -> SCALE x SCALE block
const GLYPH_W: i32 = 5 * SCALE;
const GLYPH_H: i32 = 7 * SCALE;
const PAD: i32 = SCALE; // padding inside the badge

fn put(img: &mut RgbaImage, x: i32, y: i32, c: Rgba<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
        img.put_pixel(x as u32, y as u32, c);
    }
}

fn fill_rect(img: &mut RgbaImage, x0: i32, y0: i32, w: i32, h: i32, c: Rgba<u8>) {
    for dy in 0..h {
        for dx in 0..w {
            put(img, x0 + dx, y0 + dy, c);
        }
    }
}

/// Draw a single scaled digit at (x, y) in `color`.
fn draw_digit(img: &mut RgbaImage, digit: usize, x: i32, y: i32, color: Rgba<u8>) {
    let glyph = &DIGITS[digit.min(9)];
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5 {
            if bits & (1 << (4 - col)) != 0 {
                fill_rect(img, x + col * SCALE, y + row as i32 * SCALE, SCALE, SCALE, color);
            }
        }
    }
}

/// Pixel width a label of `digits` digits occupies (badge inner content).
fn label_width(n_digits: i32) -> i32 {
    n_digits * GLYPH_W + (n_digits - 1) * SCALE
}

/// Draw all marks onto `img` (mutating it). Badges are red with white digits and
/// a thin white border, placed with the top-left anchored at the mark position
/// but nudged fully on-screen. Overlapping badges are drawn in order.
pub fn draw_marks(img: &mut RgbaImage, marks: &[Mark]) {
    let red = Rgba([220u8, 30, 30, 255]);
    let white = Rgba([255u8, 255, 255, 255]);
    let (iw, ih) = (img.width() as i32, img.height() as i32);

    for m in marks {
        let s = m.label.to_string();
        let ndig = s.len() as i32;
        let bw = label_width(ndig) + 2 * PAD;
        let bh = GLYPH_H + 2 * PAD;
        // Anchor top-left at the element corner, clamped inside the image.
        let bx = m.x.clamp(0, (iw - bw).max(0));
        let by = m.y.clamp(0, (ih - bh).max(0));
        // Badge background + border.
        fill_rect(img, bx, by, bw, bh, white);
        fill_rect(img, bx + 1, by + 1, bw - 2, bh - 2, red);
        // Digits.
        let mut dx = bx + PAD;
        for ch in s.chars() {
            let d = ch.to_digit(10).unwrap_or(0) as usize;
            draw_digit(img, d, dx, by + PAD, white);
            dx += GLYPH_W + SCALE;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_marks_changes_pixels_at_mark() {
        let mut img = RgbaImage::from_pixel(200, 200, Rgba([0, 0, 0, 255]));
        draw_marks(&mut img, &[Mark { label: 7, x: 50, y: 60 }]);
        // Somewhere in the badge region should now be non-black.
        let mut changed = false;
        for y in 60..(60 + GLYPH_H as u32 + 4) {
            for x in 50..(50 + 40u32) {
                if img.get_pixel(x, y).0 != [0, 0, 0, 255] {
                    changed = true;
                }
            }
        }
        assert!(changed, "badge must draw visible pixels");
    }

    #[test]
    fn draw_marks_clamps_offscreen_position() {
        // A mark past the edge must still land inside the image (no panic).
        let mut img = RgbaImage::from_pixel(60, 60, Rgba([0, 0, 0, 255]));
        draw_marks(&mut img, &[Mark { label: 42, x: 1000, y: 1000 }]);
        // Bottom-right region should have badge pixels.
        let mut changed = false;
        for y in 0..60u32 {
            for x in 0..60u32 {
                if img.get_pixel(x, y).0 != [0, 0, 0, 255] {
                    changed = true;
                }
            }
        }
        assert!(changed, "offscreen mark must be clamped in-bounds and drawn");
    }

    #[test]
    fn multi_digit_label_wider_than_single() {
        assert!(label_width(2) > label_width(1));
    }
}
