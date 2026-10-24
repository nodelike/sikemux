//! What's New: the notes of the release running here, who made it and how much went
//! into it, beside a sky in the release's colours.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, relative, AnyElement, App, Context, Entity, FocusHandle, FontWeight, Hsla, Image,
    ImageFormat, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window,
};
use gpui_transcript::{Markdown, Pen, Style};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::core::{self, Ask};
use crate::experience::{self, Experience};
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

/// Contributors shown with their names; the rest are faces in a wall.
const FEATURED: usize = 3;
const WALL: usize = 13;

#[derive(Default)]
pub struct WhatsNew {
    /// The release running here, which closing the screen counts as read.
    running: Option<String>,
    /// The release shown: an update waiting to install, else the one running.
    version: Option<String>,
    /// The person asked to update now and is being told which agents that stops.
    confirming: bool,
    notes: Option<Result<Notes, String>>,
    markdown: Option<Rc<Markdown>>,
    avatars: HashMap<String, Arc<Image>>,
    everyone: bool,
    /// Holds the keys while it is up, so Escape closes it and the shell's shortcuts stand aside.
    focus: Option<FocusHandle>,
}

/// Takes the keys the first time it is drawn.
pub fn take_keys(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    if let Some(news) = news_of(bench).filter(|news| news.focus.is_none()) {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        news.focus = Some(focus);
    }
}

impl WhatsNew {
    pub fn running(&self) -> Option<String> {
        self.running.clone()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Notes {
    notes: Option<String>,
    date: Option<String>,
    commits: Option<u32>,
    compare: Option<String>,
    #[serde(default)]
    contributors: Vec<Contributor>,
}

#[derive(Clone, Deserialize)]
struct Contributor {
    login: String,
    name: String,
    commits: u32,
    avatar: String,
}

fn news_of(bench: &mut Workbench) -> Option<&mut WhatsNew> {
    match bench.experience.as_mut()? {
        Experience::WhatsNew(news) => Some(news),
        _ => None,
    }
}

/// Asks the core which release runs here, then for its notes and its contributors' faces.
pub fn opened(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let running = core::request(&asks, "whatsNew", json!({})).await;
        let Some(version) = running
            .ok()
            .and_then(|news| news["version"].as_str().map(str::to_owned))
        else {
            return;
        };
        let Ok(version) = bench.update(cx, |bench, cx| {
            let shown = pending(bench)
                .and_then(|update| update["available"]["version"].as_str())
                .map_or_else(|| version.clone(), str::to_owned);
            if let Some(news) = news_of(bench) {
                news.running = Some(version);
                news.version = Some(shown.clone());
            }
            bench.parts.redraw(&bench.parts.over, cx);
            shown
        }) else {
            return;
        };
        let notes = core::request(&asks, "releaseNotes", json!({ "version": version }))
            .await
            .and_then(|notes| {
                serde_json::from_value::<Notes>(notes).map_err(|error| error.to_string())
            });
        let urls: Vec<String> = notes
            .as_ref()
            .map(|notes| {
                notes
                    .contributors
                    .iter()
                    .map(|person| person.avatar.clone())
                    .collect()
            })
            .unwrap_or_default();
        let _ = bench.update(cx, |bench, cx| {
            if let Some(news) = news_of(bench) {
                news.markdown = notes
                    .as_ref()
                    .ok()
                    .and_then(|notes| notes.notes.as_deref())
                    .map(|text| Rc::new(crate::editor::preview::render(&without_title(text))));
                news.notes = Some(notes);
            }
            bench.parts.redraw(&bench.parts.over, cx);
        });
        if urls.is_empty() {
            return;
        }
        let faces = core::request(&asks, "releaseAvatars", json!({ "urls": urls }))
            .await
            .unwrap_or_default();
        let _ = bench.update(cx, |bench, cx| {
            if let Some(news) = news_of(bench) {
                let faces = faces.as_object().into_iter().flatten();
                news.avatars = faces
                    .filter_map(|(url, data)| Some((url.clone(), data_image(data.as_str()?)?)))
                    .collect();
            }
            bench.parts.redraw(&bench.parts.over, cx);
        });
    })
    .detach();
}

/// The update the core found for this app, while one is there to install.
fn pending(bench: &Workbench) -> Option<&Value> {
    bench
        .read("update")
        .filter(|update| update["updates"] == true && !update["available"].is_null())
}

fn waiting_label(count: usize) -> String {
    format!(
        "Update waiting on {count} agent{}",
        if count == 1 { "" } else { "s" }
    )
}

/// `2048` as `2.0 KB`, `15728640` as `15 MB`.
fn bytes(count: u64) -> String {
    let mut size = count as f64;
    for unit in ["KB", "MB", "GB", "TB"] {
        size /= 1024.;
        if size < 1024. || unit == "TB" {
            return match size < 10. {
                true => format!("{size:.1} {unit}"),
                false => format!("{size:.0} {unit}"),
            };
        }
    }
    unreachable!()
}

/// What the install button says for where the update stands, and whether it can be pressed.
fn install_button(update: &Value) -> (String, bool) {
    let version = update["available"]["version"].as_str().unwrap_or_default();
    let blockers = update["blockers"].as_array().map_or(0, Vec::len);
    match update["state"].as_str().unwrap_or_default() {
        "downloading" => {
            let done = update["downloadedBytes"].as_u64().unwrap_or(0);
            let label = match update["totalBytes"].as_u64().filter(|total| *total > 0) {
                Some(total) => format!("Downloading · {}%", (done * 100 / total).min(100)),
                None if done > 0 => format!("Downloading · {}", bytes(done)),
                None => "Downloading…".to_owned(),
            };
            (label, false)
        }
        "installing" => ("Verifying & installing…".to_owned(), false),
        "ready" => ("Restarting…".to_owned(), false),
        "waiting" => (waiting_label(blockers), false),
        "failed" => (format!("Retry v{version}"), true),
        _ => (format!("Install v{version}"), true),
    }
}

fn install(bench: &Workbench, force: bool) {
    let _ = bench.asks.send(Ask::Call {
        method: "updateInstall",
        params: json!({ "force": force }),
    });
}

/// The notes as they read under the screen's own heading, without the release's title.
fn without_title(notes: &str) -> String {
    notes
        .lines()
        .filter(|line| !line.starts_with("# "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A `data:` address the core handed over, as a picture.
fn data_image(data: &str) -> Option<Arc<Image>> {
    use base64::Engine;
    let (head, body) = data.strip_prefix("data:")?.split_once(";base64,")?;
    let format = ImageFormat::from_mime_type(head)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

/// `2026-10-21T…` as `21 Oct 2026`.
fn release_date(date: &str) -> Option<String> {
    let date = chrono::DateTime::parse_from_rfc3339(date).ok()?;
    Some(date.format("%-d %b %Y").to_string())
}

/// `0.4.0-nightly.10` as its release and its build: `0.4.0` and `nightly 10`.
fn split_version(version: &str) -> (String, Option<String>) {
    let Some((release, build)) = version.split_once('-') else {
        return (version.to_owned(), None);
    };
    let build = match build.split_once('.') {
        Some((name, number)) => format!("{name} {number}"),
        None => build.to_owned(),
    };
    (release.to_owned(), Some(build))
}

fn plural(count: u32, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

fn profile(login: &str) -> String {
    format!("https://github.com/{login}")
}

pub fn render(
    bench: &Workbench,
    this: &Entity<Workbench>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let Some(Experience::WhatsNew(news)) = &bench.experience else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let close = {
        let this = this.clone();
        move |_: &gpui_kit::MouseDownEvent, _: &mut Window, cx: &mut App| {
            this.update(cx, experience::close)
        }
    };
    let card = div()
        .id("whats-new")
        .w(px(920.))
        .max_w_full()
        .h(px(640.))
        .max_h_full()
        .flex()
        .overflow_hidden()
        .bg(t.surface)
        .border_1()
        .border_color(t.border)
        .rounded(theme::RADIUS_4)
        .shadow(t.shadow_float())
        .text_color(t.text_secondary)
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| {
            cx.stop_propagation()
        })
        .child(side(news, pending(bench), this, cx))
        .child(main(news, pending(bench), this, window, cx));
    div()
        .id("whats-new-backdrop")
        .key_context("WhatsNew Modal")
        .when_some(news.focus.as_ref(), |backdrop, focus| {
            backdrop.track_focus(focus)
        })
        .absolute()
        .inset_0()
        .p(px(24.))
        .flex()
        .items_center()
        .justify_center()
        .bg(Hsla {
            a: 0.76,
            ..t.surface_sunken
        })
        .occlude()
        .on_mouse_down(gpui_kit::MouseButton::Left, close)
        .on_key_down({
            let this = this.clone();
            move |event, _, cx| {
                if event.keystroke.key == "escape" {
                    this.update(cx, experience::close);
                }
            }
        })
        .child(card)
        .into_any_element()
}

fn side(
    news: &WhatsNew,
    update: Option<&Value>,
    this: &Entity<Workbench>,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx);
    let notes = news.notes.as_ref().and_then(|notes| notes.as_ref().ok());
    let version = news.version.clone().unwrap_or_default();
    let (release, build) = split_version(&version);
    let channel = match (update.is_some(), version.contains('-')) {
        (true, _) => "update ready",
        (false, true) => "nightly",
        (false, false) => "stable",
    };
    let highlights = notes
        .and_then(|notes| notes.notes.as_deref())
        .map_or(0, |text| {
            text.lines()
                .filter(|line| line.starts_with("- ") || line.starts_with("* "))
                .count() as u32
        });
    let people = notes.map_or(&[][..], |notes| notes.contributors.as_slice());
    let stat = |name: &'static str, value: u32| {
        div()
            .flex()
            .flex_col()
            .child(
                ui::text(value.to_string(), 15., FontWeight::SEMIBOLD)
                    .mb(px(2.))
                    .text_color(t.text_primary),
            )
            .child(ui::text(name, 12., FontWeight::NORMAL).text_color(t.text_tertiary))
    };
    div()
        .id("whats-new-side")
        .relative()
        .w(px(300.))
        .flex_none()
        .flex()
        .flex_col()
        .pt(px(132.))
        .px(px(22.))
        .pb(px(20.))
        .overflow_y_scroll()
        .border_r_1()
        .border_color(t.border)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .h(px(240.))
                .child(crate::ui::grain::sky(0.15, 1., cx).size_full())
                .children(ui::spot(|| "whats-new-sky".into())),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .self_start()
                .px(px(7.))
                .py(px(4.))
                .rounded(theme::RADIUS_1)
                .bg(Hsla { a: 0.12, ..t.live })
                .text_color(t.live)
                .child(div().size(px(5.)).rounded_full().bg(t.live))
                .child(
                    ui::text(channel.to_uppercase(), 10., FontWeight::SEMIBOLD)
                        .letter_spacing(0.08),
                ),
        )
        .child(
            div()
                .mt(px(20.))
                .flex()
                .text_size(crate::theme::ui_text(48.))
                .font_weight(FontWeight::BOLD)
                .line_height(relative(1.))
                .letter_spacing(-0.06)
                .text_color(t.text_primary)
                .child(div().text_color(t.accent).child("v"))
                .child(if release.is_empty() {
                    "…".to_owned()
                } else {
                    release
                }),
        )
        .children(build.map(|build| {
            ui::text(build, 20., FontWeight::SEMIBOLD)
                .mt(px(8.))
                .letter_spacing(-0.02)
                .text_color(t.text_secondary)
        }))
        .children(
            notes
                .and_then(|notes| notes.date.as_deref())
                .and_then(release_date)
                .map(|date| {
                    ui::text(date, 12., FontWeight::NORMAL)
                        .mt(px(10.))
                        .text_color(t.text_tertiary)
                }),
        )
        .child(
            div()
                .flex()
                .gap(px(22.))
                .mt(px(22.))
                .children(
                    notes
                        .and_then(|notes| notes.commits)
                        .map(|commits| stat("commits", commits)),
                )
                .when(!people.is_empty(), |stats| {
                    stats.child(stat("people", people.len() as u32))
                })
                .when(highlights > 0, |stats| {
                    stats.child(stat("highlights", highlights))
                }),
        )
        .when(!people.is_empty(), |side| {
            side.child(contributors(news, people, this, cx))
        })
}

fn avatar(news: &WhatsNew, person: &Contributor, size: f32, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    match news.avatars.get(&person.avatar) {
        Some(face) => img(face.clone())
            .size(px(size))
            .flex_none()
            .rounded_full()
            .into_any_element(),
        None => div()
            .size(px(size))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(t.border)
            .text_size(px(if size > 29. { 12. } else { 10. }))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.text_secondary)
            .child(
                person
                    .name
                    .chars()
                    .next()
                    .or(person.login.chars().next())
                    .map(|first| first.to_uppercase().to_string())
                    .unwrap_or_default(),
            )
            .into_any_element(),
    }
}

fn contributors(
    news: &WhatsNew,
    people: &[Contributor],
    this: &Entity<Workbench>,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let open = |login: &str| {
        let url = profile(login);
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| cx.open_url(&url)
    };
    let (featured, rest) = people.split_at(people.len().min(FEATURED));
    let shown = if news.everyone {
        rest
    } else {
        &rest[..rest.len().min(WALL)]
    };
    div()
        .mt_auto()
        .pt(px(22.))
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            ui::text("CONTRIBUTORS", 10., FontWeight::SEMIBOLD)
                .mb(px(6.))
                .letter_spacing(0.14)
                .text_color(t.text_tertiary),
        )
        .children(featured.iter().map(|person| {
            div()
                .id(SharedString::from(format!("contributor-{}", person.login)))
                .group("contributor")
                .flex()
                .items_center()
                .gap(px(10.))
                .min_w_0()
                .px(px(10.))
                .py(px(9.))
                .border_1()
                .border_color(gpui_kit::transparent_black())
                .rounded(theme::RADIUS_2)
                .bg(t.surface_overlay)
                .cursor_pointer()
                .hover(|row| row.border_color(t.border).bg(t.surface_raised))
                .on_click(open(&person.login))
                .child(avatar(news, person, 30., cx))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .min_w_0()
                        .child(
                            ui::text(person.name.clone(), 13., FontWeight::SEMIBOLD)
                                .overflow_hidden()
                                .text_ellipsis()
                                .text_color(t.text_primary),
                        )
                        .child(
                            ui::text(
                                format!("@{} · {}", person.login, plural(person.commits, "commit")),
                                11.,
                                FontWeight::NORMAL,
                            )
                            .text_color(t.text_tertiary)
                            .group_hover("contributor", |text| text.text_color(t.accent)),
                        ),
                )
        }))
        .when(!rest.is_empty(), |list| {
            list.child(
                ui::text(format!("and {} more", rest.len()), 12., FontWeight::NORMAL)
                    .mt(px(10.))
                    .text_color(t.text_tertiary),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(5.))
                    .mt(px(4.))
                    .children(shown.iter().map(|person| {
                        div()
                            .id(SharedString::from(format!("face-{}", person.login)))
                            .rounded_full()
                            .cursor_pointer()
                            .tooltip(ui::tooltip(format!(
                                "@{} · {}",
                                person.login,
                                plural(person.commits, "commit")
                            )))
                            .on_click(open(&person.login))
                            .child(avatar(news, person, 28., cx))
                    }))
                    .when(shown.len() < rest.len(), |wall| {
                        let this = this.clone();
                        wall.child(
                            div()
                                .id("contributors-rest")
                                .h(px(28.))
                                .min_w(px(28.))
                                .px(px(8.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(theme::RADIUS_2)
                                .bg(t.border)
                                .text_size(crate::theme::ui_text(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text_secondary)
                                .cursor_pointer()
                                .hover(|rest| rest.text_color(t.text_primary).bg(t.surface_hover))
                                .on_click(move |_, _, cx| {
                                    this.update(cx, |bench, cx| {
                                        if let Some(news) = news_of(bench) {
                                            news.everyone = true;
                                        }
                                        bench.parts.redraw(&bench.parts.over, cx);
                                    })
                                })
                                .child(format!("+{}", rest.len() - shown.len())),
                        )
                    }),
            )
        })
}

fn main(
    news: &WhatsNew,
    update: Option<&Value>,
    this: &Entity<Workbench>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let version = news.version.clone().unwrap_or_default();
    let notes = news.notes.as_ref();
    let status = |text: String| {
        div()
            .text_size(crate::theme::ui_text(13.5))
            .line_height(relative(1.6))
            .text_color(t.text_tertiary)
            .child(text)
    };
    let body: AnyElement = match (&news.markdown, notes) {
        (Some(markdown), _) => notes_markdown(markdown, &t, window, cx),
        (None, Some(Err(error))) => status(format!(
            "The notes for v{version} could not be loaded. {error}"
        ))
        .into_any_element(),
        (None, Some(Ok(_))) => {
            status(format!("v{version} shipped without notes.")).into_any_element()
        }
        (None, None) => status(format!("Loading the notes for v{version}…")).into_any_element(),
    };
    let compare = notes
        .and_then(|notes| notes.as_ref().ok())
        .and_then(|notes| notes.compare.clone());
    let esc = {
        let this = this.clone();
        div()
            .id("whats-new-esc")
            .px(px(7.))
            .py(px(5.))
            .border_1()
            .border_color(t.border)
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(10.))
            .font_weight(FontWeight::MEDIUM)
            .line_height(crate::theme::ui_text(10.))
            .text_color(t.text_tertiary)
            .cursor_pointer()
            .hover(|esc| esc.text_color(t.text_primary).border_color(t.border_strong))
            .on_click(move |_, _, cx| this.update(cx, experience::close))
            .child("esc")
    };
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pl(px(32.))
                .pr(px(24.))
                .py(px(18.))
                .border_b_1()
                .border_color(t.border)
                .child(ui::text("What’s new", 15., FontWeight::SEMIBOLD).text_color(t.text_primary))
                .child(esc),
        )
        .child(
            div()
                .id("whats-new-notes")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .pl(px(32.))
                .pr(px(40.))
                .pt(px(24.))
                .pb(px(32.))
                .child(body),
        )
        .children(
            update
                .filter(|update| update["state"] == "waiting")
                .map(|update| waiting(news, update, this, cx)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .pl(px(32.))
                .pr(px(20.))
                .py(px(12.))
                .border_t_1()
                .border_color(t.border)
                .child(match compare {
                    Some(compare) => {
                        let range = compare
                            .split("/compare/")
                            .nth(1)
                            .map(|range| range.replace("...", "…"))
                            .unwrap_or_else(|| "Compare".to_owned());
                        div()
                            .id("whats-new-compare")
                            .text_size(crate::theme::ui_text(12.))
                            .text_color(t.text_tertiary)
                            .cursor_pointer()
                            .hover(|link| link.text_color(t.live))
                            .on_click(move |_, _, cx| cx.open_url(&compare))
                            .child(format!("{range} ↗"))
                            .into_any_element()
                    }
                    None => div().into_any_element(),
                })
                .children(update.map(|update| {
                    let (label, pressable) = install_button(update);
                    let this = this.clone();
                    ui::Button::new("whats-new-install", label, ui::Variant::Primary)
                        .disabled(!pressable)
                        .on_click(move |_, _, cx| this.update(cx, |bench, _| install(bench, false)))
                })),
        )
}

/// An update held back by agents that cannot be carried over it yet, with a way to
/// install now that names the agents it would stop.
fn waiting(
    news: &WhatsNew,
    update: &Value,
    this: &Entity<Workbench>,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let blockers = update["blockers"].as_array().map_or(&[][..], Vec::as_slice);
    let count = blockers.len();
    let one = count == 1;
    let them = if one { "it" } else { "them" };
    let paragraph = |text: String| div().line_height(relative(1.5)).child(text);
    let confirm = |confirming: bool| {
        let this = this.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            this.update(cx, |bench, cx| {
                if let Some(news) = news_of(bench) {
                    news.confirming = confirming;
                }
                bench.parts.redraw(&bench.parts.over, cx);
            })
        }
    };
    let not_now = {
        let this = this.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            this.update(cx, |bench, _| {
                let _ = bench.asks.send(Ask::Call {
                    method: "updateCancel",
                    params: json!({}),
                });
            })
        }
    };
    let force = {
        let this = this.clone();
        move |_: &gpui_kit::ClickEvent, _: &mut Window, cx: &mut App| {
            this.update(cx, |bench, _| install(bench, true))
        }
    };
    let action = |id: &'static str, label: String, danger: bool| {
        div()
            .id(id)
            .px(px(12.))
            .py(px(7.))
            .border_1()
            .border_color(if danger {
                Hsla { a: 0.5, ..t.danger }
            } else {
                t.border
            })
            .rounded(theme::RADIUS_2)
            .text_color(if danger { t.danger } else { t.text_secondary })
            .cursor_pointer()
            .when(danger, |button| {
                button.hover(|button| {
                    button.bg(Hsla {
                        a: 0.12,
                        ..t.danger
                    })
                })
            })
            .child(label)
    };
    let keep = match news.confirming {
        true => {
            action("whats-new-keep-waiting", "Keep waiting".into(), false).on_click(confirm(false))
        }
        false => action("whats-new-not-now", "Not now".into(), false).on_click(not_now),
    };
    let go = match news.confirming {
        true => action(
            "whats-new-force",
            match one {
                true => "Stop it and update".into(),
                false => format!("Stop {count} agents and update"),
            },
            true,
        )
        .on_click(force),
        false => action("whats-new-ask-force", "Force update".into(), true).on_click(confirm(true)),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .pl(px(32.))
        .pr(px(20.))
        .py(px(12.))
        .border_t_1()
        .border_color(t.border)
        .text_size(crate::theme::ui_text(12.))
        .text_color(t.text_secondary)
        .child(
            div()
                .line_height(relative(1.5))
                .child(
                    div()
                        .text_color(t.warn)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(waiting_label(count)),
                )
                .child(format!(
                    "Agents at work keep running through an update, but {} still starting or in the middle of a request. The update installs on its own once {} ready.",
                    if one { "this one is" } else { "these are" },
                    if one { "it is" } else { "they are" },
                )),
        )
        .when(news.confirming, |section| {
            section
                .child(div().flex().flex_col().gap(px(6.)).children(blockers.iter().map(|blocker| {
                    let title = blocker["title"]
                        .as_str()
                        .or(blocker["agentId"].as_str())
                        .unwrap_or_default()
                        .to_owned();
                    let state = match blocker["reason"].as_str() {
                        Some("starting") => "starting",
                        _ => "busy",
                    };
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .gap(px(12.))
                        .min_w_0()
                        .px(px(10.))
                        .py(px(7.))
                        .border_1()
                        .border_color(t.border)
                        .rounded(theme::RADIUS_2)
                        .bg(t.surface_raised)
                        .child(div().min_w_0().truncate().text_color(t.text_primary).child(title))
                        .child(
                            div()
                                .flex_none()
                                .text_color(t.text_tertiary)
                                .child(format!("{} · {state}", blocker["provider"].as_str().unwrap_or_default())),
                        )
                })))
                .child(paragraph(format!(
                    "Updating now stops {them} and starts {them} again on the same chat. Whatever {} doing stops.",
                    if one { "it was" } else { "they were" },
                )).text_color(t.text_tertiary))
        })
        .child(div().flex().justify_end().gap(px(8.)).child(keep).child(go))
}

fn notes_markdown(
    markdown: &Rc<Markdown>,
    t: &Tokens,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let mut mono = gpui_kit::font(theme::MONO);
    mono.fallbacks = Some(gpui_kit::FontFallbacks::from_fonts(vec![
        theme::MONO_ICONS.into()
    ]));
    let style = Style {
        font: theme::UI.into(),
        mono,
        text_size: px(13.5),
        line_height: 1.6,
        ink: t.text_secondary,
        ink_dim: t.text_tertiary,
        ink_faint: t.text_disabled,
        accent: t.live,
        line: t.border,
        line_soft: t.line_soft,
        selection: Hsla { a: 0.3, ..t.accent },
        mark_ground: Hsla { a: 0.1, ..t.accent },
        table_ground: Hsla {
            a: 0.72,
            ..t.line_soft
        },
        code_ground: t.surface_raised,
        numbered_sections: true,
        on_link: Some(Rc::new(|link: &str, _: &mut Window, cx: &mut App| {
            if link.starts_with("https://") || link.starts_with("http://") {
                cx.open_url(link);
            }
        })),
        ..Style::default()
    };
    let pen = Pen::new(Rc::new(style));
    pen.markdown(
        SharedString::from("whats-new-markdown"),
        markdown,
        window,
        cx,
    )
    .into_any_element()
}
