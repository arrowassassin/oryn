//! Design tokens, transcribed verbatim from the Claude Design handoff
//! (`Oryn.dc.html`). Pure data — no GPUI types — so the palette is testable and
//! independent of any rendering API. Solid colors are `0xRRGGBB`; translucent
//! overlays are `(white_or_black, alpha)` pairs resolved at paint time.
//!
//! Two themes (dark default + light) and five swappable accents, matching the
//! design's Appearance settings.

/// A 0xRRGGBB color.
pub type Rgb = u32;

/// An overlay color: a base (white in dark mode, black in light) plus an alpha
/// in `[0,1]`. The design uses these for hairlines, fills, and hovers so they
/// composite over whatever surface they sit on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overlay {
    pub base: Rgb,
    pub alpha: f32,
}

impl Overlay {
    const fn new(base: Rgb, alpha: f32) -> Self {
        Self { base, alpha }
    }
}

/// The white-alpha overlay ramp (`--w015` … `--w18` in the design).
#[derive(Debug, Clone, Copy)]
pub struct Overlays {
    pub w015: Overlay,
    pub w02: Overlay,
    pub w025: Overlay,
    pub w03: Overlay,
    pub w035: Overlay,
    pub w04: Overlay,
    pub w05: Overlay,
    pub w06: Overlay,
    pub w07: Overlay,
    pub w08: Overlay,
    pub w09: Overlay,
    pub w10: Overlay,
    pub w12: Overlay,
    pub w13: Overlay,
    pub w18: Overlay,
}

/// Foreground text ramp (`--t1` brightest … `--t7` faintest).
#[derive(Debug, Clone, Copy)]
pub struct TextRamp {
    pub t1: Rgb,
    pub t2: Rgb,
    pub t3: Rgb,
    pub t4: Rgb,
    pub t5: Rgb,
    pub t6: Rgb,
    pub t7: Rgb,
}

/// Surface ramp, dark→light by elevation.
#[derive(Debug, Clone, Copy)]
pub struct Surfaces {
    pub bg: Rgb,
    pub bg2: Rgb,
    pub bg3: Rgb,
    pub surf3: Rgb,
    pub inset: Rgb,
    pub panel: Rgb,
    pub raised: Rgb,
    pub dot_faint: Rgb,
}

/// Semantic foreground colors (status text, diff text).
#[derive(Debug, Clone, Copy)]
pub struct Semantic {
    pub ok_fg: Rgb,
    pub info_fg: Rgb,
    pub err_fg: Rgb,
    pub warn_fg: Rgb,
    pub diff_add_fg: Rgb,
    pub diff_del_fg: Rgb,
    pub diff_link_fg: Rgb,
    pub ok_bg: Rgb,
}

/// Fixed status hues used for dots, pills, and gauges across both themes
/// (the design uses these literal hex values inline regardless of theme).
#[derive(Debug, Clone, Copy)]
pub struct Status {
    /// running / success / added
    pub green: Rgb,
    /// finished / info / provenance
    pub blue: Rgb,
    /// stopped / error / removed
    pub red: Rgb,
    /// warning / drift
    pub amber: Rgb,
}

pub const STATUS: Status = Status {
    green: 0x4ED99A,
    blue: 0x7FA8FF,
    red: 0xFF6B6B,
    amber: 0xFFB454,
};

/// An accent with its base, a brighter hover variant, and a deep variant used
/// for accent-colored foreground text in light mode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Accent {
    pub name: &'static str,
    pub base: Rgb,
    pub bright: Rgb,
    pub deep: Rgb,
}

/// The five accents offered in Appearance settings.
pub const ACCENTS: [Accent; 5] = [
    Accent {
        name: "Violet",
        base: 0xC08CFF,
        bright: 0xD4ADFF,
        deep: 0x7C3AED,
    },
    Accent {
        name: "Blue",
        base: 0x7FA8FF,
        bright: 0xA6C6FF,
        deep: 0x2563EB,
    },
    Accent {
        name: "Green",
        base: 0x4ED99A,
        bright: 0x73E6B4,
        deep: 0x0F9D58,
    },
    Accent {
        name: "Ember",
        base: 0xFF7A45,
        bright: 0xFF9468,
        deep: 0xD9531F,
    },
    Accent {
        name: "Amber",
        base: 0xFFB454,
        bright: 0xFFC97E,
        deep: 0xB5780E,
    },
];

/// Whether the app is in dark or light mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

/// A fully-resolved theme: surfaces, text, semantics, overlays, and the active
/// accent for the current [`Mode`].
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub mode: Mode,
    pub surfaces: Surfaces,
    pub text: TextRamp,
    pub semantic: Semantic,
    pub overlays: Overlays,
    pub status: Status,
    pub accent: Accent,
}

impl Theme {
    /// The dark theme (the design's default) with `accent`.
    pub fn dark(accent: Accent) -> Self {
        Self {
            mode: Mode::Dark,
            surfaces: Surfaces {
                bg: 0x08080A,
                bg2: 0x0A0A0D,
                bg3: 0x0B0B0E,
                surf3: 0x0C0C10,
                inset: 0x0D0D11,
                panel: 0x0E0E12,
                raised: 0x101015,
                dot_faint: 0x2C2C33,
            },
            text: TextRamp {
                t1: 0xECECEF,
                t2: 0xC9C9D2,
                t3: 0x9C9CA8,
                t4: 0x8B8B95,
                t5: 0x65656F,
                t6: 0x4A4A53,
                t7: 0x3A3A42,
            },
            semantic: Semantic {
                ok_fg: 0x9FE6C4,
                info_fg: 0xBCD2FF,
                err_fg: 0xFFB0B0,
                warn_fg: 0xFFCE8E,
                diff_add_fg: 0xBFE9D3,
                diff_del_fg: 0xF0BCBC,
                diff_link_fg: 0xD3C2EF,
                ok_bg: 0x0F1A14,
            },
            overlays: Overlays {
                w015: Overlay::new(0xFFFFFF, 0.015),
                w02: Overlay::new(0xFFFFFF, 0.022),
                w025: Overlay::new(0xFFFFFF, 0.025),
                w03: Overlay::new(0xFFFFFF, 0.03),
                w035: Overlay::new(0xFFFFFF, 0.035),
                w04: Overlay::new(0xFFFFFF, 0.04),
                w05: Overlay::new(0xFFFFFF, 0.05),
                w06: Overlay::new(0xFFFFFF, 0.06),
                w07: Overlay::new(0xFFFFFF, 0.07),
                w08: Overlay::new(0xFFFFFF, 0.08),
                w09: Overlay::new(0xFFFFFF, 0.09),
                w10: Overlay::new(0xFFFFFF, 0.10),
                w12: Overlay::new(0xFFFFFF, 0.12),
                w13: Overlay::new(0xFFFFFF, 0.13),
                w18: Overlay::new(0xFFFFFF, 0.18),
            },
            status: STATUS,
            accent,
        }
    }

    /// The light theme with `accent`.
    pub fn light(accent: Accent) -> Self {
        Self {
            mode: Mode::Light,
            surfaces: Surfaces {
                bg: 0xF4F4F6,
                bg2: 0xEBEBED,
                bg3: 0xF8F8FA,
                surf3: 0xEEEEF1,
                inset: 0xF1F1F4,
                panel: 0xFFFFFF,
                raised: 0xFFFFFF,
                dot_faint: 0xC4C4CC,
            },
            text: TextRamp {
                t1: 0x16161B,
                t2: 0x34343D,
                t3: 0x54545D,
                t4: 0x67676F,
                t5: 0x78787F,
                t6: 0x9C9CA4,
                t7: 0xBCBCC4,
            },
            semantic: Semantic {
                ok_fg: 0x15794A,
                info_fg: 0x2056B5,
                err_fg: 0xC0271F,
                warn_fg: 0x8A5A12,
                diff_add_fg: 0x18794A,
                diff_del_fg: 0xB32820,
                diff_link_fg: 0x6B3FB0,
                ok_bg: 0xE7F6EE,
            },
            overlays: Overlays {
                w015: Overlay::new(0x000000, 0.03),
                w02: Overlay::new(0x000000, 0.035),
                w025: Overlay::new(0x000000, 0.04),
                w03: Overlay::new(0x000000, 0.045),
                w035: Overlay::new(0x000000, 0.05),
                w04: Overlay::new(0x000000, 0.055),
                w05: Overlay::new(0x000000, 0.07),
                w06: Overlay::new(0x000000, 0.09),
                w07: Overlay::new(0x000000, 0.10),
                w08: Overlay::new(0x000000, 0.11),
                w09: Overlay::new(0x000000, 0.12),
                w10: Overlay::new(0x000000, 0.14),
                w12: Overlay::new(0x000000, 0.16),
                w13: Overlay::new(0x000000, 0.10),
                w18: Overlay::new(0x000000, 0.24),
            },
            status: STATUS,
            accent,
        }
    }

    /// Resolve a theme for `mode` with `accent`.
    pub fn resolve(mode: Mode, accent: Accent) -> Self {
        match mode {
            Mode::Dark => Self::dark(accent),
            Mode::Light => Self::light(accent),
        }
    }

    /// Accent foreground: in light mode the design uses the deep variant for
    /// legible accent-colored text; in dark mode the base.
    pub fn accent_fg(&self) -> Rgb {
        match self.mode {
            Mode::Dark => self.accent.base,
            Mode::Light => self.accent.deep,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark(ACCENTS[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_dark_violet() {
        let t = Theme::default();
        assert_eq!(t.mode, Mode::Dark);
        assert_eq!(t.surfaces.bg, 0x08080A);
        assert_eq!(t.accent.base, 0xC08CFF);
        assert_eq!(t.accent_fg(), 0xC08CFF);
    }

    #[test]
    fn light_accent_fg_uses_deep_variant() {
        let t = Theme::light(ACCENTS[0]);
        assert_eq!(t.mode, Mode::Light);
        assert_eq!(t.surfaces.bg, 0xF4F4F6);
        assert_eq!(t.accent_fg(), 0x7C3AED);
    }

    #[test]
    fn resolve_matches_constructors() {
        let a = ACCENTS[2];
        assert_eq!(
            Theme::resolve(Mode::Dark, a).surfaces.bg,
            Theme::dark(a).surfaces.bg
        );
        assert_eq!(
            Theme::resolve(Mode::Light, a).surfaces.bg,
            Theme::light(a).surfaces.bg
        );
    }

    #[test]
    fn five_distinct_accents() {
        let mut bases: Vec<Rgb> = ACCENTS.iter().map(|a| a.base).collect();
        bases.sort_unstable();
        bases.dedup();
        assert_eq!(bases.len(), 5);
    }

    #[test]
    fn status_hues_match_design() {
        assert_eq!(STATUS.green, 0x4ED99A);
        assert_eq!(STATUS.blue, 0x7FA8FF);
        assert_eq!(STATUS.red, 0xFF6B6B);
        assert_eq!(STATUS.amber, 0xFFB454);
    }
}
