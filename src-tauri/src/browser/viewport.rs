//! A tab can be held at a fixed viewport, whatever size the pane is. The page
//! lays out at exactly that size, and its view is drawn scaled down to fit the
//! pane, never up, centred along the top of it with the pane showing around it.

use std::time::{Duration, Instant};

use serde::Serialize;

use super::{BrowserBounds, BrowserHole, BrowserManager, MOBILE_USER_AGENT};

const MIN_SIZE: u32 = 200;
const MAX_SIZE: u32 = 4000;
/// A pane this narrow or short is collapsed or still being laid out, not showing a page.
const MIN_SHOWN: f64 = 40.0;
const SETTLE_TIMEOUT: Duration = Duration::from_secs(3);
const SETTLE_POLL: Duration = Duration::from_millis(50);

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

    /// Sites pick their phone layout by the agent string, so the mobile preset
    /// introduces itself as an iPhone. An iPad's Safari already claims to be a Mac.
    pub fn user_agent(&self) -> Option<&'static str> {
        (self.preset == Some("mobile")).then_some(MOBILE_USER_AGENT)
    }

    fn size(&self) -> (f64, f64) {
        (f64::from(self.width), f64::from(self.height))
    }
}

/// Where a tab's view goes and the size its page lays out at, in CSS pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Layout {
    /// In the pane at `frame`, in the window's coordinates. With `page`, the
    /// page lays out at that size and is drawn scaled into `frame`. The clips
    /// and holes are always in the page's own pixels.
    Shown {
        frame: BrowserBounds,
        page: Option<(f64, f64)>,
    },
    /// Out of sight, still laid out at `page` so the agent reads a real layout.
    Parked { page: (f64, f64) },
}

impl Layout {
    pub fn page_size(&self) -> (f64, f64) {
        match self {
            Layout::Shown {
                page: Some(page), ..
            }
            | Layout::Parked { page } => *page,
            Layout::Shown { frame, page: None } => (frame.width, frame.height),
        }
    }
}

/// `area` is the pane's page area when this tab is the one it shows, and
/// `last_seen` the size that area last had, which a parked tab keeps.
pub fn layout(
    area: Option<&BrowserBounds>,
    fixed: Option<Viewport>,
    last_seen: (f64, f64),
) -> Layout {
    match (area, fixed) {
        (Some(area), Some(fixed)) => Layout::Shown {
            frame: fit(area, fixed),
            page: Some(fixed.size()),
        },
        (Some(area), None) => Layout::Shown {
            frame: area.clone(),
            page: None,
        },
        (None, Some(fixed)) => Layout::Parked { page: fixed.size() },
        (None, None) => Layout::Parked { page: last_seen },
    }
}

/// A pane that has shrunk to nothing, as it does while hidden or collapsed.
pub fn collapsed(area: &BrowserBounds) -> bool {
    area.width < MIN_SHOWN || area.height < MIN_SHOWN
}

/// Where the scaled page goes inside the pane's page area.
fn fit(area: &BrowserBounds, viewport: Viewport) -> BrowserBounds {
    let (width, height) = viewport.size();
    let scale = (area.width / width).min(area.height / height).min(1.0);
    let (placed_width, placed_height) = (width * scale, height * scale);
    let x = area.x + (area.width - placed_width) / 2.0;
    let y = area.y;
    let seen_from = area.x + area.clip_left;
    let seen_to = area.x + area.width - area.clip_right;
    let clip_left = (seen_from - x).clamp(0.0, placed_width);
    let clip_right = (x + placed_width - seen_to).clamp(0.0, placed_width - clip_left);
    BrowserBounds {
        x,
        y,
        width: placed_width,
        height: placed_height,
        clip_left: clip_left / scale,
        clip_right: clip_right / scale,
        holes: area
            .holes
            .iter()
            .map(|hole| BrowserHole {
                x: (hole.x - (x - area.x)) / scale,
                y: (hole.y - (y - area.y)) / scale,
                width: hole.width / scale,
                height: hole.height / scale,
                radius: hole.radius / scale,
            })
            .collect(),
        dim: area.dim,
    }
}

/// Whether the page's own `innerWidth` and `innerHeight` match `expected`.
/// They are whole numbers, so a fractional pane rounds either way.
fn matches(reported: (f64, f64), expected: (f64, f64)) -> bool {
    (reported.0 - expected.0).abs() < 1.0 && (reported.1 - expected.1).abs() < 1.0
}

fn parse_inner_size(raw: &str) -> Option<(f64, f64)> {
    let text = match serde_json::from_str::<serde_json::Value>(raw).ok()? {
        serde_json::Value::String(text) => text,
        _ => return None,
    };
    let (width, height) = text.split_once('x')?;
    Some((width.parse().ok()?, height.parse().ok()?))
}
impl BrowserManager {
    /// The size the tab's page should lay out at now, in CSS pixels.
    pub fn layout_size(&self, agent_id: &str, tab_id: &str) -> Option<(f64, f64)> {
        let agents = self.lock();
        let agent = agents.get(agent_id)?;
        agent.views.get(tab_id)?;
        Some(agent.layout_of(tab_id).page_size())
    }

    /// Waits until the page itself reports the size it should lay out at, and
    /// returns that size. Call it after `set_viewport` and any reload it caused.
    pub async fn settle_viewport(
        &self,
        agent_id: &str,
        tab_id: &str,
    ) -> Result<(u32, u32), String> {
        let started = Instant::now();
        let mut last = None;
        loop {
            let expected = self
                .layout_size(agent_id, tab_id)
                .ok_or("the tab closed while its viewport was changing")?;
            let view = self
                .lock()
                .get(agent_id)
                .and_then(|agent| agent.views.get(tab_id).cloned())
                .ok_or("the tab closed while its viewport was changing")?;
            if let Ok(raw) = super::tools::eval(&view, r#"`${innerWidth}x${innerHeight}`"#).await {
                if let Some(reported) = parse_inner_size(&raw) {
                    if matches(reported, expected) {
                        return Ok((reported.0 as u32, reported.1 as u32));
                    }
                    last = Some(reported);
                }
            }
            if started.elapsed() >= SETTLE_TIMEOUT {
                return Err(match last {
                    Some((width, height)) => format!(
                        "the page still lays out at {width}x{height}, not the {}x{} asked for",
                        expected.0, expected.1
                    ),
                    None => format!(
                        "the page did not report its size, so {}x{} could not be confirmed",
                        expected.0, expected.1
                    ),
                });
            }
            tokio::time::sleep(SETTLE_POLL).await;
        }
    }
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
            dim: 0.0,
        }
    }

    const SEEN: (f64, f64) = (900.0, 700.0);

    #[test]
    fn a_wide_viewport_lays_out_at_its_size_and_is_drawn_at_half_in_a_narrow_pane() {
        let desktop = Viewport::preset("desktop").unwrap();
        let Layout::Shown { frame, page } = layout(Some(&area(640.0, 900.0)), Some(desktop), SEEN)
        else {
            panic!("the shown tab was parked");
        };
        assert_eq!(page, Some((1280.0, 800.0)));
        assert_eq!((frame.width, frame.height), (640.0, 400.0));
        assert_eq!((frame.x, frame.y), (100.0, 50.0));
    }

    #[test]
    fn a_small_viewport_keeps_its_size_and_sits_centred_along_the_top() {
        let mobile = Viewport::preset("mobile").unwrap();
        let frame = fit(&area(1200.0, 900.0), mobile);
        assert_eq!((frame.width, frame.height), (390.0, 844.0));
        assert_eq!(frame.x, 100.0 + (1200.0 - 390.0) / 2.0);
        assert_eq!(frame.y, 50.0);
    }

    #[test]
    fn a_tall_viewport_is_fitted_by_its_height() {
        let tablet = Viewport::preset("tablet").unwrap();
        let frame = fit(&area(1200.0, 590.0), tablet);
        assert_eq!((frame.width, frame.height), (410.0, 590.0));
    }

    #[test]
    fn clips_and_holes_move_into_the_page_s_own_pixels() {
        let mut pane = area(1000.0, 1000.0);
        pane.clip_left = 400.0;
        pane.holes.push(BrowserHole {
            x: 500.0,
            y: 20.0,
            width: 40.0,
            height: 40.0,
            radius: 6.0,
        });
        let frame = fit(&pane, Viewport::sized(400, 400).unwrap());
        assert_eq!(frame.x, 400.0);
        assert_eq!(frame.clip_left, 100.0);
        assert_eq!(frame.clip_right, 0.0);
        assert_eq!((frame.holes[0].x, frame.holes[0].y), (200.0, 20.0));

        let halved = fit(&pane, Viewport::sized(2000, 2000).unwrap());
        assert_eq!(halved.width, 1000.0);
        assert_eq!(halved.clip_left, 800.0);
        assert_eq!(
            (
                halved.holes[0].x,
                halved.holes[0].width,
                halved.holes[0].radius
            ),
            (1000.0, 80.0, 12.0)
        );
    }

    #[test]
    fn a_tab_that_follows_the_pane_takes_the_pane_as_it_is() {
        let pane = area(640.0, 480.0);
        let shown = layout(Some(&pane), None, SEEN);
        assert_eq!(
            shown,
            Layout::Shown {
                frame: pane,
                page: None
            }
        );
        assert_eq!(shown.page_size(), (640.0, 480.0));
    }

    #[test]
    fn a_parked_tab_lays_out_at_its_viewport_or_the_pane_s_last_real_size() {
        let mobile = Viewport::preset("mobile").unwrap();
        assert_eq!(
            layout(None, Some(mobile), SEEN),
            Layout::Parked {
                page: (390.0, 844.0)
            }
        );
        assert_eq!(layout(None, None, SEEN), Layout::Parked { page: SEEN });
    }

    #[test]
    fn a_pane_shrunk_to_a_sliver_counts_as_collapsed() {
        assert!(collapsed(&area(1.0, 1.0)));
        assert!(collapsed(&area(900.0, 12.0)));
        assert!(!collapsed(&area(320.0, 240.0)));
    }

    #[test]
    fn the_page_s_reported_size_is_read_and_matched_to_the_pixel() {
        assert_eq!(parse_inner_size(r#""1280x800""#), Some((1280.0, 800.0)));
        assert_eq!(parse_inner_size("1280x800"), None);
        assert_eq!(parse_inner_size(r#""nonsense""#), None);
        assert!(matches((640.0, 480.0), (640.4, 479.6)));
        assert!(!matches((1280.0, 1280.0), (1280.0, 800.0)));
    }

    #[test]
    fn sizes_outside_the_range_are_refused() {
        assert!(Viewport::sized(100, 800).is_err());
        assert!(Viewport::sized(1280, 5000).is_err());
        assert!(Viewport::preset("watch").is_none());
    }

    #[test]
    fn only_the_mobile_preset_claims_to_be_a_phone() {
        assert_eq!(
            Viewport::preset("mobile").unwrap().user_agent(),
            Some(MOBILE_USER_AGENT)
        );
        assert_eq!(Viewport::preset("tablet").unwrap().user_agent(), None);
        assert_eq!(Viewport::sized(390, 844).unwrap().user_agent(), None);
    }
}
