//! A tab can be held at a fixed viewport, whatever size the pane is. The page
//! is laid out at that size and scaled down to fit the pane, never up, and
//! centred along the top of it with the pane showing around it.

use serde::Serialize;

use super::{BrowserBounds, BrowserHole};

const MIN_SIZE: u32 = 200;
const MAX_SIZE: u32 = 4000;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub width: u32,
    pub height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<&'static str>,
}

impl Viewport {
    pub fn sized(width: u32, height: u32) -> Result<Self, String> {
        let allowed = MIN_SIZE..=MAX_SIZE;
        if !allowed.contains(&width) || !allowed.contains(&height) {
            return Err(format!(
                "width and height must each be between {MIN_SIZE} and {MAX_SIZE}"
            ));
        }
        Ok(Self {
            width,
            height,
            preset: None,
        })
    }

    pub fn preset(name: &str) -> Option<Self> {
        let (preset, width, height) = match name {
            "desktop" => ("desktop", 1280, 800),
            "tablet" => ("tablet", 820, 1180),
            "mobile" => ("mobile", 390, 844),
            _ => return None,
        };
        Some(Self {
            width,
            height,
            preset: Some(preset),
        })
    }
}

/// Where the page goes inside the pane's page area, and the zoom that lays it
/// out at the viewport's size. Clips and holes move into the page's own frame.
pub fn fit(area: &BrowserBounds, viewport: Viewport) -> (BrowserBounds, f64) {
    let (width, height) = (f64::from(viewport.width), f64::from(viewport.height));
    let zoom = (area.width / width).min(area.height / height).min(1.0);
    let (placed_width, placed_height) = (width * zoom, height * zoom);
    let x = area.x + (area.width - placed_width) / 2.0;
    let y = area.y;
    let seen_from = area.x + area.clip_left;
    let seen_to = area.x + area.width - area.clip_right;
    let clip_left = (seen_from - x).clamp(0.0, placed_width);
    let clip_right = (x + placed_width - seen_to).clamp(0.0, placed_width - clip_left);
    let placed = BrowserBounds {
        x,
        y,
        width: placed_width,
        height: placed_height,
        clip_left,
        clip_right,
        holes: area
            .holes
            .iter()
            .map(|hole| BrowserHole {
                x: hole.x - (x - area.x),
                y: hole.y - (y - area.y),
                ..*hole
            })
            .collect(),
    };
    (placed, zoom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(width: f64, height: f64) -> BrowserBounds {
        BrowserBounds {
            x: 100.0,
            y: 50.0,
            width,
            height,
            clip_left: 0.0,
            clip_right: 0.0,
            holes: Vec::new(),
        }
    }

    #[test]
    fn a_wide_viewport_in_a_narrow_pane_is_scaled_down_to_fit() {
        let desktop = Viewport::preset("desktop").unwrap();
        let (placed, zoom) = fit(&area(640.0, 900.0), desktop);
        assert_eq!(zoom, 0.5);
        assert_eq!((placed.width, placed.height), (640.0, 400.0));
        assert_eq!((placed.x, placed.y), (100.0, 50.0));
        assert_eq!(placed.width / zoom, 1280.0);
    }

    #[test]
    fn a_small_viewport_keeps_its_size_and_sits_centred_along_the_top() {
        let mobile = Viewport::preset("mobile").unwrap();
        let (placed, zoom) = fit(&area(1200.0, 900.0), mobile);
        assert_eq!(zoom, 1.0);
        assert_eq!((placed.width, placed.height), (390.0, 844.0));
        assert_eq!(placed.x, 100.0 + (1200.0 - 390.0) / 2.0);
        assert_eq!(placed.y, 50.0);
    }

    #[test]
    fn a_tall_viewport_is_fitted_by_its_height() {
        let tablet = Viewport::preset("tablet").unwrap();
        let (placed, zoom) = fit(&area(1200.0, 590.0), tablet);
        assert_eq!(zoom, 0.5);
        assert_eq!((placed.width, placed.height), (410.0, 590.0));
    }

    #[test]
    fn clips_and_holes_follow_the_page_into_its_own_frame() {
        let mut pane = area(1000.0, 1000.0);
        pane.clip_left = 400.0;
        pane.holes.push(BrowserHole {
            x: 500.0,
            y: 20.0,
            width: 40.0,
            height: 40.0,
            radius: 6.0,
        });
        let (placed, _) = fit(&pane, Viewport::sized(400, 400).unwrap());
        assert_eq!(placed.x, 400.0);
        assert_eq!(placed.clip_left, 100.0);
        assert_eq!(placed.clip_right, 0.0);
        assert_eq!((placed.holes[0].x, placed.holes[0].y), (200.0, 20.0));
    }

    #[test]
    fn sizes_outside_the_range_are_refused() {
        assert!(Viewport::sized(100, 800).is_err());
        assert!(Viewport::sized(1280, 5000).is_err());
        assert!(Viewport::preset("watch").is_none());
    }
}
