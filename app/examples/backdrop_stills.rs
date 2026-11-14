//! Draws each backdrop kind's still frame offscreen at 2x, as a pane draws it with motion
//! reduced, for `scripts/backdrop-stills.mjs` to compare with the phones' shaders:
//!   cargo run --example backdrop_stills -- <folder> <picture>
//! writes grain.png at 390x422 points and picture.png at 390x506, the picture covering it all,
//! and the theme's colours they took to inks.json.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    div, px, size, AppContext, HeadlessAppContext, IntoElement, ParentElement, Render, Styled,
};
use sikemux_gpui::{icons, theme, ui::grain};
use sikemux_view::prefs::{PaneCoverage, Preferences};

struct Pane;

impl Render for Pane {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().relative().size_full().children(grain::field(cx))
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let [_, folder, picture] = args.as_slice() else {
        anyhow::bail!("usage: backdrop_stills <folder> <picture>");
    };
    std::fs::create_dir_all(folder)?;
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::bench_text_system(),
        Arc::new(icons::Icons),
        gpui_platform::current_headless_renderer,
    );
    cx.update(|cx| {
        gpui_kit::init(cx);
        theme::install(cx);
        cx.set_reduce_motion(true);
    });
    let inks = cx.update(|cx| {
        let tokens = theme::Tokens::get(cx);
        let rgba = |colour: gpui::Hsla| {
            let colour = colour.to_rgb();
            [colour.r, colour.g, colour.b, colour.a]
        };
        serde_json::json!({
            "dot": rgba(tokens.shader_dot),
            "accent": rgba(tokens.accent),
            "text": rgba(tokens.text_primary),
        })
    });
    std::fs::write(
        std::path::Path::new(folder).join("inks.json"),
        inks.to_string(),
    )?;
    let pictured = Preferences {
        pane_image: Some(picture.clone()),
        pane_coverage: PaneCoverage::Full,
        ..Preferences::default()
    };
    for (name, height, preferences) in [
        ("grain", 422., Preferences::default()),
        ("picture", 506., pictured),
    ] {
        cx.update(|cx| grain::set(&preferences, cx));
        let started = Instant::now();
        while cx.update(|cx| grain::field(cx).is_none()) {
            anyhow::ensure!(
                started.elapsed() < Duration::from_secs(30),
                "the {name} never came ready"
            );
            cx.run_until_parked();
            std::thread::sleep(Duration::from_millis(50));
        }
        let window = cx.update(|cx| {
            cx.open_window(
                gpui::WindowOptions {
                    window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds {
                        origin: gpui::point(px(0.), px(0.)),
                        size: size(px(390.), px(height)),
                    })),
                    focus: false,
                    show: false,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Pane),
            )
        })?;
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))?;
        cx.capture_screenshot(window.into())?
            .save(std::path::Path::new(folder).join(format!("{name}.png")))?;
    }
    Ok(())
}
