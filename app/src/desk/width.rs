//! The width a desk's page lays out at: the chip at the right of its address bar, and All,
//! the live page in one column beside pictures of it at the other two widths.

use std::collections::{HashMap, HashSet, VecDeque};
use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, relative, AnyElement, Context, FontWeight, Image, ImageFormat,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Transformation,
};
use sikemux_browser::viewport::Viewport;
use sikemux_browser::BrowserTab;

use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_2, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS_H, TEXT_XS};
use crate::ui::{self, MenuRow};
use crate::workbench::Workbench;

/// The fixed widths, each by the preset the browser knows it as, its name and its icon.
const WIDTHS: [(&str, &str, Icon); 3] = [
    ("mobile", "Phone", Icon::Phone),
    ("tablet", "Tablet", Icon::Tablet),
    ("desktop", "Desktop", Icon::Laptop),
];

/// How many pages' pictures are kept, the least recently taken let go first.
const KEPT_PAGES: usize = 4;

/// The widest a picture is kept, in pixels: the column it is drawn in, at twice its points.
const PICTURE_PIXELS: u32 = 640;

/// A picture this much taller than wide is cut off below, so two fit the column.
const TALLEST_PICTURE: f32 = 1.05;

/// Pictures of pages at each width, by address, for All.
#[derive(Default)]
pub(crate) struct Pictures {
    by_address: HashMap<String, HashMap<&'static str, Picture>>,
    order: VecDeque<String>,
    taking: HashSet<String>,
    /// Pages to take again once the taking under way ends, as they loaded anew meanwhile.
    again: HashSet<String>,
    /// Tabs shown with All whose page was loading when last heard, taken again once it ends.
    loading: HashSet<String>,
    /// Each tab whose page loaded anew, waiting for it to stay still before it is taken again.
    settling: HashMap<String, gpui_kit::Task<()>>,
}

struct Picture {
    image: Arc<Image>,
    taken: u64,
}

impl Pictures {
    fn keep(&mut self, url: &str, shots: Vec<(&'static str, Vec<u8>)>, taken: u64) {
        let pictures = self.by_address.entry(url.to_owned()).or_default();
        for (preset, jpeg) in shots {
            let image = Arc::new(Image::from_bytes(ImageFormat::Jpeg, jpeg));
            pictures.insert(preset, Picture { image, taken });
        }
        self.order.retain(|kept| kept != url);
        self.order.push_back(url.to_owned());
        while self.order.len() > KEPT_PAGES {
            if let Some(oldest) = self.order.pop_front() {
                self.by_address.remove(&oldest);
            }
        }
    }
}

fn preset_of(held: Option<Viewport>) -> Option<&'static str> {
    held.and_then(|held| held.preset)
}

/// The width All keeps live: the one the tab is held at, or the phone's when it follows the pane.
fn live_preset(held: Option<Viewport>) -> &'static str {
    preset_of(held)
        .filter(|preset| WIDTHS.iter().any(|(known, ..)| known == preset))
        .unwrap_or("mobile")
}

fn named(preset: &str) -> (&'static str, Icon) {
    WIDTHS
        .iter()
        .find(|(known, ..)| *known == preset)
        .map_or(("Fit", Icon::Resize), |(_, name, icon)| (*name, *icon))
}

/// The chip at the right of the address bar: the width's icon and name, and a menu of the
/// widths and All.
pub(super) fn chip(
    bench: &Workbench,
    agent: &str,
    tab_id: &str,
    cx: &Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let engine = bench.desks.read(cx).engine.clone();
    let held = engine
        .as_ref()
        .and_then(|engine| engine.viewport(agent, tab_id));
    let all = engine
        .as_ref()
        .is_some_and(|engine| engine.all_widths(agent, tab_id));
    let (icon, label): (Icon, SharedString) = match (all, held) {
        (true, _) => (Icon::Viewport, "All".into()),
        (false, None) => (Icon::Resize, "Fit".into()),
        (false, Some(held)) => match held.preset {
            Some(preset) => {
                let (name, icon) = named(preset);
                (icon, format!("{name} {}", held.width).into())
            }
            None => (
                Icon::Resize,
                format!("{}×{}", held.width, held.height).into(),
            ),
        },
    };
    let face = BaseButton::new("desk-width-face")
        .accessibility_label(format!("Width: {label}"))
        .flex_none()
        .h(px(20.))
        .pl(px(7.))
        .pr(SPACE_1H)
        .mr(px(3.))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded(RADIUS_2)
        .bg(t.surface_chosen)
        .hover(move |chip| chip.bg(t.surface_active))
        .cursor_pointer()
        .text_size(crate::theme::ui_text(TEXT_XS))
        .text_color(t.text_primary)
        .whitespace_nowrap()
        .child(ui::icon(icon, 12.).text_color(t.text_secondary))
        .child(label)
        .child(
            ui::icon(Icon::Chevron, 10.)
                .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                .text_color(t.text_tertiary),
        );
    let chosen = move |preset: Option<&str>| !all && preset_of(held) == preset;
    let (bench, agent, tab_id) = (cx.entity().downgrade(), agent.to_owned(), tab_id.to_owned());
    ui::menu_button(
        "desk-width",
        gpui_kit::Anchor::TopRight,
        face,
        move |mut popup, _, cx| {
            popup = popup.min_w(px(200.));
            let fit = std::iter::once((None, "Fit", Icon::Resize));
            let fixed = WIDTHS
                .iter()
                .map(|(preset, name, icon)| (Some(*preset), *name, *icon));
            for (preset, name, icon) in fit.chain(fixed) {
                let size = preset.and_then(Viewport::preset);
                let (bench, agent, tab_id) = (bench.clone(), agent.clone(), tab_id.clone());
                popup = popup.item(
                    MenuRow::new(name)
                        .icon(icon)
                        .when_some(size, |row, size| {
                            row.hint(format!("{}×{}", size.width, size.height))
                        })
                        .chosen(chosen(preset))
                        .item(cx)
                        .on_click(move |_, _, cx| {
                            crate::keymap::counted("browser.width", cx);
                            bench
                                .update(cx, |bench, cx| {
                                    bench.with_engine(cx, |engine| {
                                        engine.set_all_widths(&agent, &tab_id, false);
                                        engine.set_viewport(&agent, &tab_id, size)
                                    });
                                    cx.notify();
                                })
                                .ok();
                        }),
                );
            }
            let (bench, agent, tab_id) = (bench.clone(), agent.clone(), tab_id.clone());
            popup.separator().item(
                MenuRow::new("All three")
                    .icon(Icon::Viewport)
                    .chosen(all)
                    .item(cx)
                    .on_click(move |_, _, cx| {
                        crate::keymap::counted("browser.widthAll", cx);
                        bench
                            .update(cx, |bench, cx| {
                                bench.with_engine(cx, |engine| {
                                    engine.set_all_widths(&agent, &tab_id, true);
                                    match engine.viewport(&agent, &tab_id) {
                                        Some(held) if preset_of(Some(held)).is_some() => Ok(()),
                                        _ => engine.set_viewport(
                                            &agent,
                                            &tab_id,
                                            Viewport::preset("mobile"),
                                        ),
                                    }
                                });
                                bench.take_pictures(&agent, cx);
                                cx.notify();
                            })
                            .ok();
                    }),
            )
        },
    )
    .into_any_element()
}

/// Whether the desk shows `tab` with All.
pub(super) fn shows_all(
    bench: &Workbench,
    agent: &str,
    tab: &BrowserTab,
    cx: &Context<Workbench>,
) -> bool {
    (bench.desks.read(cx).engine.as_ref()).is_some_and(|engine| engine.all_widths(agent, &tab.id))
}

/// All: `live`, the page itself at its width, in one column with its name, and pictures of the
/// page at the other two widths stacked beside it, each made the live one by a click.
pub(super) fn all(
    bench: &Workbench,
    agent: &str,
    tab: &BrowserTab,
    live: AnyElement,
    cx: &Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let desks = bench.desks.read(cx);
    let held = (desks.engine.as_ref()).and_then(|engine| engine.viewport(agent, &tab.id));
    let live_at = live_preset(held);
    let pictures = desks.pictures.by_address.get(&tab.url);
    let label = |name: &str, after: String| {
        div()
            .flex()
            .items_center()
            .child(ui::caps(name, cx).text_color(t.text_secondary))
            .child(ui::caps(&after, cx))
    };
    let live_column = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap(SPACE_1H)
                .child(div().size(px(6.)).rounded_full().bg(t.live))
                .child(label(named(live_at).0, " · live".to_owned())),
        )
        .child(div().flex_1().min_h_0().child(live));
    let others =
        WIDTHS
            .iter()
            .filter(|(preset, ..)| *preset != live_at)
            .map(|(preset, name, _)| {
                let size = Viewport::preset(preset).expect("every width is a preset");
                let picture = pictures.and_then(|pictures| pictures.get(preset));
                let after = picture.map_or_else(String::new, |picture| {
                    format!(" · {}", ui::time::clock(picture.taken))
                });
                let shape = size.width as f32 / size.height as f32;
                let (bench, agent, tab_id) =
                    (cx.entity().downgrade(), agent.to_owned(), tab.id.clone());
                div()
                    .id(SharedString::from(format!("desk-width-picture-{preset}")))
                    .flex()
                    .flex_col()
                    .gap(SPACE_1H)
                    .cursor_pointer()
                    .group("desk-width-picture")
                    .child(label(name, after))
                    .child(
                        div()
                            .w_full()
                            .aspect_ratio(shape.max(1. / TALLEST_PICTURE))
                            .overflow_hidden()
                            .rounded(px(6.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.surface_raised)
                            .hover(move |frame| frame.border_color(t.border_strong))
                            .map(|frame| match picture {
                                Some(picture) => frame.child(
                                    div()
                                        .w_full()
                                        .aspect_ratio(shape)
                                        .child(gpui_kit::img(picture.image.clone()).size_full()),
                                ),
                                None => frame.flex().items_center().justify_center().child(
                                    gpui_kit::component::spinner::Spinner::new()
                                        .with_size(px(13.))
                                        .color(t.text_tertiary),
                                ),
                            }),
                    )
                    .on_click(move |_, _, cx| {
                        bench
                            .update(cx, |bench, cx| {
                                bench.with_engine(cx, |engine| {
                                    engine.set_viewport(&agent, &tab_id, Viewport::preset(preset))
                                });
                                bench.take_pictures(&agent, cx);
                                cx.notify();
                            })
                            .ok();
                    })
            });
    let pictures_column = div()
        .flex_none()
        .w(relative(0.42))
        .max_w(px(300.))
        .flex()
        .flex_col()
        .gap(SPACE_3)
        .children(others)
        .child(
            ui::text(
                "Taken by Sikemux. Click one to try it live.",
                TEXT_2XS_H,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        );
    div()
        .size_full()
        .flex()
        .gap(SPACE_3)
        .px(SPACE_3)
        .pt(SPACE_2)
        .pb(SPACE_3)
        .child(live_column)
        .child(pictures_column)
        .into_any_element()
}

/// `jpeg` at most [`PICTURE_PIXELS`] wide, so a desktop-wide picture is not kept at full size.
fn shrunk(jpeg: Vec<u8>) -> Vec<u8> {
    let Ok(picture) = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) else {
        return jpeg;
    };
    if picture.width() <= PICTURE_PIXELS {
        return jpeg;
    }
    let height = picture.height() * PICTURE_PIXELS / picture.width();
    let smaller = picture.resize_exact(
        PICTURE_PIXELS,
        height,
        image::imageops::FilterType::Triangle,
    );
    let mut out = Vec::new();
    match smaller.to_rgb8().write_to(
        &mut std::io::Cursor::new(&mut out),
        image::ImageFormat::Jpeg,
    ) {
        Ok(()) => out,
        Err(_) => jpeg,
    }
}

/// How long a page must go without loading again before its pictures are taken again, so a
/// dev server reloading on every save takes them once its burst is over.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

impl Workbench {
    /// Keeps the pictures All shows beside the page in front on `agent`'s desk: takes the widths
    /// it has none of yet, and all of them again once the page has loaded anew and settled.
    pub(crate) fn take_pictures(&mut self, agent: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.all_widths_tab(agent, cx) else {
            return;
        };
        let anew = self.desks.update(cx, |desks, _| {
            let pictures = &mut desks.pictures;
            if tab.loading {
                pictures.loading.insert(tab.id.clone());
                pictures.settling.remove(&tab.id);
                return None;
            }
            Some(pictures.loading.remove(&tab.id))
        });
        match anew {
            None => {}
            Some(false) => self.take(agent, false, cx),
            Some(true) => {
                let owner = agent.to_owned();
                let settled = cx.spawn(async move |bench, cx| {
                    cx.background_executor().timer(SETTLE).await;
                    bench
                        .update(cx, |bench, cx| bench.take(&owner, true, cx))
                        .ok();
                });
                self.desks.update(cx, |desks, _| {
                    desks.pictures.settling.insert(tab.id.clone(), settled)
                });
            }
        }
    }

    /// The page in front on `agent`'s desk, while it is shown with All.
    fn all_widths_tab(&self, agent: &str, cx: &Context<Self>) -> Option<BrowserTab> {
        let desks = self.desks.read(cx);
        let engine = desks.engine.as_ref()?;
        let tab = (desks.strips.get(agent))
            .and_then(|strip| strip.tabs.iter().find(|tab| tab.active).cloned())?;
        (engine.all_widths(agent, &tab.id) && !super::idle_blank(&tab)).then_some(tab)
    }

    /// Takes the page in front at the widths All shows beside it that it has no picture of, or
    /// with `fresh` at all of them.
    fn take(&mut self, agent: &str, fresh: bool, cx: &mut Context<Self>) {
        let (Some(tab), Some(engine)) = (
            self.all_widths_tab(agent, cx),
            self.desks.read(cx).engine.clone(),
        ) else {
            return;
        };
        if tab.loading {
            return;
        }
        let live_at = live_preset(engine.viewport(agent, &tab.id));
        let wanted = self.desks.update(cx, |desks, _| {
            let pictures = &mut desks.pictures;
            pictures.settling.remove(&tab.id);
            if pictures.taking.contains(&tab.url) {
                if fresh {
                    pictures.again.insert(tab.url.clone());
                }
                return Vec::new();
            }
            if fresh {
                if let Some(had) = pictures.by_address.get_mut(&tab.url) {
                    had.remove(live_at);
                }
            }
            let had = pictures.by_address.get(&tab.url);
            let wanted: Vec<&'static str> = WIDTHS
                .iter()
                .map(|(preset, ..)| *preset)
                .filter(|preset| *preset != live_at)
                .filter(|preset| fresh || had.is_none_or(|had| !had.contains_key(preset)))
                .collect();
            if !wanted.is_empty() {
                pictures.taking.insert(tab.url.clone());
            }
            wanted
        });
        if wanted.is_empty() {
            return;
        }
        let (owner, url) = (agent.to_owned(), tab.url.clone());
        cx.spawn(async move |bench, cx| {
            let (taking, agent) = (url.clone(), owner.clone());
            let shots = super::run(async move {
                let mut shots = Vec::new();
                for preset in wanted {
                    let Some(fixed) = Viewport::preset(preset) else {
                        continue;
                    };
                    match engine.picture_at(&agent, &taking, fixed).await {
                        Ok(jpeg) => shots.push((preset, shrunk(jpeg))),
                        Err(error) => {
                            eprintln!(
                                "sikemux: the page was not taken at the {preset} width: {error}"
                            )
                        }
                    }
                }
                shots
            })
            .await
            .unwrap_or_default();
            bench
                .update(cx, |bench, cx| {
                    let again = bench.desks.update(cx, |desks, cx| {
                        let pictures = &mut desks.pictures;
                        pictures.taking.remove(&url);
                        pictures.keep(&url, shots, ui::time::now_ms());
                        cx.notify();
                        pictures.again.remove(&url)
                    });
                    if again {
                        bench.take(&owner, true, cx);
                    }
                })
                .ok();
        })
        .detach();
    }
}
