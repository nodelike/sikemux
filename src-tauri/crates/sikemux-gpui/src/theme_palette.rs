//! The theme palette: every theme, each worn by the window while it is
//! highlighted, and the person's own put back when they leave without one.

use gpui_kit::{Context, SharedString, Window};
use serde_json::json;
use sikemux_wire::theme::{Palette, ThemeSource, ThemeSummary};

use crate::core;
use crate::palette::{Row, Spec};
use crate::theme;
use crate::workbench::Workbench;

#[derive(Clone, Default, PartialEq, gpui::Action)]
#[action(name = "theme.switch")]
pub struct SwitchTheme;

const ID: &str = "theme.switch";

fn rows(themes: Vec<ThemeSummary>) -> Vec<Row> {
    let mut themes = themes;
    themes.sort_by_key(|summary| match summary.source {
        ThemeSource::Custom => 0,
        ThemeSource::Sikemux => 1,
        ThemeSource::Ghostty => 2,
    });
    themes
        .into_iter()
        .map(|summary| {
            let group = match summary.source {
                ThemeSource::Custom => "Yours",
                ThemeSource::Sikemux => "Sikemux",
                ThemeSource::Ghostty => "Ghostty",
            };
            Row::new(summary.id, summary.name)
                .meta(if summary.dark { "dark" } else { "light" })
                .group(group)
        })
        .collect()
}

impl Workbench {
    pub fn toggle_theme_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current: SharedString = self.preferences().theme_id.into();
        let spec = Spec::new(ID, "pick a theme…", Vec::new(), |row, bench, _, _| {
            bench.call(
                "setPreference",
                json!({ "key": "themeId", "value": row.key.to_string() }),
            );
        })
        .start(current)
        .empty("reading themes…")
        .width(440.)
        .browse(|row, bench, _, cx| bench.wear_theme(Some(row.key.clone()), cx))
        .cancel(|bench, _, cx| bench.wear_theme(None, cx));
        self.toggle_palette(spec, window, cx);
        let listed = core::request(&self.asks, "themes", json!({}));
        cx.spawn(async move |this, cx| {
            let Ok(themes) = listed.await else {
                return;
            };
            let Ok(themes) = serde_json::from_value::<Vec<ThemeSummary>>(themes) else {
                return;
            };
            let _ = this.update(cx, |bench, cx| {
                if let Some(palette) = bench.palette.as_mut().filter(|palette| palette.id() == ID) {
                    palette.refill(rows(themes), cx);
                    bench.parts.redraw(&bench.parts.finder, cx);
                }
            });
        })
        .detach();
    }

    /// Draws the window in theme `id`, or in the person's own theme again.
    fn wear_theme(&self, id: Option<SharedString>, cx: &mut Context<Self>) {
        let worn = core::request(&self.asks, "theme", json!({ "id": id.as_deref() }));
        cx.spawn(async move |this, cx| {
            let Ok(palette) = worn.await else {
                return;
            };
            let Ok(palette) = serde_json::from_value::<Palette>(palette) else {
                return;
            };
            let _ = this.update(cx, |bench, cx| {
                let highlighted = bench
                    .palette
                    .as_ref()
                    .filter(|open| open.id() == ID)
                    .and_then(|open| open.highlighted());
                if highlighted == id {
                    theme::apply(&palette, cx);
                    crate::syntax::restyle(cx);
                    cx.refresh_windows();
                }
            });
        })
        .detach();
    }
}
