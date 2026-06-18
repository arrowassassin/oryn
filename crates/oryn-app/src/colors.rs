//! Bridges the pure-data [`crate::theme`] tokens to GPUI color types.

use gpui::{Rgba, rgb, rgba};

use crate::theme::{Overlay, Rgb};

/// A solid `0xRRGGBB` token as a GPUI color.
pub fn solid(c: Rgb) -> Rgba {
    rgb(c)
}

/// A translucent overlay token composited as `0xRRGGBBAA`.
pub fn overlay(o: Overlay) -> Rgba {
    let a = (o.alpha * 255.0).round() as u32;
    rgba((o.base << 8) | a.min(0xFF))
}

/// A solid color at an explicit alpha in `[0,1]` — for the design's inline
/// `rgba(hex, a)` accents (status fills, glows).
pub fn tint(c: Rgb, alpha: f32) -> Rgba {
    let a = (alpha * 255.0).round() as u32;
    rgba((c << 8) | a.min(0xFF))
}
