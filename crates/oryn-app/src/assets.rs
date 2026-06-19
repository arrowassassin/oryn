//! Embedded UI icons.
//!
//! GPUI's [`gpui::svg`] element resolves its `path` through the application's
//! [`AssetSource`]. We ship a small set of monochrome, stroke-based icons inline
//! (no external files, no network) and tint them at render time via `text_color`
//! — GPUI rasterizes each SVG and uses its coverage as a mask, so the icon's own
//! stroke color only needs to be opaque.
//!
//! SVG attributes are single-quoted (valid XML) so the markup can live in plain
//! Rust string literals without escaping or raw-string `#` delimiter clashes.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// Static asset source backing every `svg().path("icons/…")` in the app.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(icon(path).map(|svg| Cow::Borrowed(svg.as_bytes())))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

/// Look up an embedded icon by its asset path (e.g. `icons/mission.svg`).
fn icon(path: &str) -> Option<&'static str> {
    ICONS
        .iter()
        .find_map(|(name, svg)| (*name == path).then_some(*svg))
}

/// Every embedded icon, keyed by the path used in `svg().path(...)`.
const ICONS: &[(&str, &str)] = &[
    ("icons/mission.svg", MISSION),
    ("icons/timeline.svg", TIMELINE),
    ("icons/review.svg", REVIEW),
    ("icons/broker.svg", BROKER),
    ("icons/launch.svg", LAUNCH),
    ("icons/settings.svg", SETTINGS),
    ("icons/you.svg", YOU),
    ("icons/search.svg", SEARCH),
];

// Lucide-style icons: 24×24 viewBox, 2px round strokes, no fill. The stroke
// color is irrelevant (GPUI tints the rasterized mask), so it is left black.

/// Target / crosshair — Mission Control.
const MISSION: &str = concat!(
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='#000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>",
    "<circle cx='12' cy='12' r='9'/>",
    "<circle cx='12' cy='12' r='5'/>",
    "<circle cx='12' cy='12' r='1.5' fill='#000'/>",
    "</svg>",
);

/// Activity pulse — Timeline.
const TIMELINE: &str = concat!(
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='#000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>",
    "<polyline points='3 12 7 12 10 4 14 20 17 12 21 12'/>",
    "</svg>",
);

/// Check inside a circle — Review.
const REVIEW: &str = concat!(
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='#000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>",
    "<circle cx='12' cy='12' r='9'/>",
    "<polyline points='8 12 11 15 16 9'/>",
    "</svg>",
);

/// Stacked database — Context Broker.
const BROKER: &str = concat!(
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='#000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>",
    "<ellipse cx='12' cy='6' rx='7' ry='3'/>",
    "<path d='M5 6v6c0 1.66 3.13 3 7 3s7-1.34 7-3V6'/>",
    "<path d='M5 12v6c0 1.66 3.13 3 7 3s7-1.34 7-3v-6'/>",
    "</svg>",
);

/// Paper plane / send — Launch.
const LAUNCH: &str = concat!(
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='#000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>",
    "<path d='M21 3 10 14'/>",
    "<path d='M21 3 14 21 10 14 3 10z'/>",
    "</svg>",
);

/// Gear — Settings.
const SETTINGS: &str = concat!(
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='#000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>",
    "<path d='M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z'/>",
    "<circle cx='12' cy='12' r='3'/>",
    "</svg>",
);

/// User silhouette — You / Profile.
const YOU: &str = concat!(
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='#000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>",
    "<path d='M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2'/>",
    "<circle cx='12' cy='7' r='4'/>",
    "</svg>",
);

/// Magnifier — command palette / search box.
const SEARCH: &str = concat!(
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='#000' stroke-width='2' stroke-linecap='round' stroke-linejoin='round'>",
    "<circle cx='11' cy='11' r='7'/>",
    "<path d='M21 21 16.65 16.65'/>",
    "</svg>",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_resolves_and_is_wellformed() {
        for (name, _) in ICONS {
            let bytes = Assets
                .load(name)
                .unwrap()
                .unwrap_or_else(|| panic!("missing asset {name}"));
            let text = std::str::from_utf8(&bytes).unwrap();
            assert!(text.starts_with("<svg"), "{name} is not an svg");
            assert!(text.trim_end().ends_with("</svg>"), "{name} unterminated");
        }
    }

    #[test]
    fn unknown_asset_is_none() {
        assert!(Assets.load("icons/does-not-exist.svg").unwrap().is_none());
    }

    #[test]
    fn list_reports_all_icons() {
        assert_eq!(Assets.list("icons").unwrap().len(), ICONS.len());
    }
}
