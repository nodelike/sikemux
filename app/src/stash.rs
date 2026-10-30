//! The account's Stash: what the person shared into Sikemux from other apps, kept on their
//! account for every device. It has a pane of its own; an item goes into
//! a chat's draft from the pane, by dragging it onto the chat, from the chip over a composer when
//! items are new, or with `&` in the draft. The chat's core puts it there.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, img, px, AnyElement, App, Bounds, Context, Div, FontWeight, InteractiveElement,
    IntoElement, ObjectFit, ParentElement, Pixels, Point, SharedString, StatefulInteractiveElement,
    Styled, StyledImage, Window,
};
use serde_json::{json, Value};

use crate::icons::Icon;
use crate::picture::{Picture, Remote};
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_3H, SPACE_4,
    SPACE_5, SPACE_6, TEXT_2XS, TEXT_2XS_H, TEXT_3XS_H, TEXT_MD, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, IconButton};
use crate::workbench::Workbench;
use sikemux_wire::chat::SentContext;
use sikemux_wire::protocol::PaneKind;

/// How long a stashed prompt can be put back from its composer.
const UNDO_FOR: std::time::Duration = std::time::Duration::from_secs(10);
/// The narrowest a card gets before the grid drops a column, and the gap between cards.
const MIN_CARD: f32 = 260.;
const GAP: f32 = 14.;
const PREVIEW: f32 = 132.;
/// How wide a card's picture is decoded, enough for the widest a card stretches to.
const PICTURE: f32 = 440.;
/// A tray row's square, and how tall the tray's rows grow before they scroll.
const THUMB: f32 = 44.;
const TRAY_TALL: f32 = 380.;
/// A tray row's words: their size, the height of each of their two lines, how many are laid out
/// at most, and the widest a pill among them grows.
const WORDS: f32 = 13.;
const LINE: f32 = 19.;
const WORDS_LAID: usize = 48;
const PILL_WIDEST: f32 = 260.;

/// A stash item being dragged onto a chat.
#[derive(Clone)]
pub struct DraggedStash {
    pub id: String,
}

/// The stash as the core last read it.
pub struct Stash<'a> {
    pub signed_in: bool,
    pub items: Vec<&'a Value>,
    seen_at: Option<&'a str>,
}

impl<'a> Stash<'a> {
    pub fn of(bench: &'a Workbench) -> Self {
        let read = bench.reads.stash.as_ref();
        Self {
            signed_in: read.is_some_and(|read| read["signedIn"].as_bool() == Some(true)),
            items: read
                .and_then(|read| read["items"].as_array())
                .map_or_else(Vec::new, |items| items.iter().collect()),
            seen_at: read.and_then(|read| read["seenAt"].as_str()),
        }
    }

    /// The items stashed since the person last looked, on any device.
    pub fn new_items(&self) -> Vec<&'a Value> {
        self.items
            .iter()
            .copied()
            .filter(
                |item| match (self.seen_at.and_then(epoch_ms), created(item)) {
                    (Some(seen), Some(at)) => at > seen,
                    _ => self.seen_at.is_none(),
                },
            )
            .collect()
    }

    /// How many items are new and still unused, for the count on the composer's button.
    pub fn fresh_count(&self) -> usize {
        match self.signed_in {
            true => self
                .new_items()
                .iter()
                .filter(|item| item["takenAt"].is_null())
                .count(),
            false => 0,
        }
    }

    /// When the newest item was stashed, as the server wrote it.
    fn newest(&self) -> Option<&'a str> {
        self.items
            .iter()
            .filter_map(|item| item["createdAt"].as_str())
            .max_by_key(|at| epoch_ms(at))
    }
}

fn epoch_ms(at: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.timestamp_millis())
}

fn created(item: &Value) -> Option<i64> {
    item["createdAt"].as_str().and_then(epoch_ms)
}

/// What an item is called: its page's title, its text's first line, or its file's name.
pub fn title(item: &Value) -> String {
    let text = |key: &str| {
        item[key]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    match item["kind"].as_str() {
        Some("link") => text("url").map(|url| {
            (item["preview"]["title"].as_str().map(str::trim))
                .filter(|title| !title.is_empty())
                .unwrap_or_else(|| {
                    url.trim_start_matches("https://")
                        .trim_start_matches("http://")
                })
                .to_owned()
        }),
        Some("text" | "prompt") => {
            text("text").map(|text| text.lines().next().unwrap_or(text).to_owned())
        }
        _ => text("name").map(str::to_owned),
    }
    .unwrap_or_else(|| "Untitled".to_owned())
}

/// Where a link goes, how many files a prompt carries or how large a file is, then how long ago it
/// was stashed.
pub fn place(item: &Value) -> String {
    let what = match item["kind"].as_str() {
        Some("link") => (item["preview"]["site"].as_str())
            .filter(|site| !site.is_empty())
            .map(str::to_owned)
            .or_else(|| site_of(item["url"].as_str()?)),
        Some("text") => None,
        Some("prompt") => match item["attachments"].as_array().map_or(0, Vec::len) {
            0 => None,
            1 => Some("1 file".to_owned()),
            files => Some(format!("{files} files")),
        },
        _ => item["size"].as_u64().map(crate::codehost::bytes),
    };
    let ago = item["createdAt"].as_str().map(crate::ui::time::ago_written);
    [what, ago]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn site_of(url: &str) -> Option<String> {
    url::Url::parse(url).ok()?.host_str().map(str::to_owned)
}

/// Whether the item's title, note, link or text holds every word of `query`, ignoring case.
pub fn matches(item: &Value, query: &str) -> bool {
    let said = [
        Some(title(item)),
        item["note"].as_str().map(str::to_owned),
        item["url"].as_str().map(str::to_owned),
        item["text"].as_str().map(str::to_owned),
        item["preview"]["site"].as_str().map(str::to_owned),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    query
        .to_lowercase()
        .split(|c: char| c.is_whitespace() || c == '&')
        .filter(|word| !word.is_empty())
        .all(|word| said.contains(word))
}

/// The glyph of an item with no picture to show: its file's, or its kind's.
pub fn glyph(item: &Value, size: f32, cx: &App) -> AnyElement {
    let ink = Tokens::get(cx).text_tertiary;
    if item["kind"].as_str() == Some("file") {
        if let Some(name) = item["name"].as_str() {
            return crate::file_icons::glyph(name, size, cx);
        }
    }
    let icon = match item["kind"].as_str() {
        Some("link") => Icon::Globe,
        Some("text") => Icon::Text,
        Some("prompt") => Icon::Message,
        Some("image") => Icon::Image,
        Some("video") => Icon::Camera,
        _ => Icon::File,
    };
    ui::icon(icon, size).text_color(ink).into_any_element()
}

/// The item's picture, or its page's, when it has one, which the core fetches.
fn picture(bench: &Workbench, item: &Value) -> Option<Picture> {
    let asks = bench.asks.clone();
    match item["kind"].as_str()? {
        "image" => Some(Picture::Remote(Remote::stashed(
            item["id"].as_str()?.to_owned(),
            asks,
        ))),
        "link" => Some(Picture::Remote(Remote::new(
            item["preview"]["imageUrl"].as_str()?.to_owned(),
            asks,
        ))),
        _ => None,
    }
}

/// The item small, as a mark beside its words: its picture, else its glyph.
pub fn mark(bench: &Workbench, item: &Value, size: f32, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let tile = div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_1)
        .overflow_hidden()
        .bg(t.surface_raised);
    match picture(bench, item) {
        Some(shown) => {
            let fallback = glyph(item, size * 0.6, cx);
            let fallback = std::cell::RefCell::new(Some(fallback));
            tile.child(
                img(shown.source_at(px(size)))
                    .size(px(size))
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        fallback
                            .borrow_mut()
                            .take()
                            .unwrap_or_else(|| div().into_any_element())
                    }),
            )
            .into_any_element()
        }
        None => tile.child(glyph(item, size * 0.66, cx)).into_any_element(),
    }
}

impl Workbench {
    /// Puts the stash items `ids` in the draft of `agent`'s chat, through the chat's core. What
    /// is typed is kept first, so the core adds them after it.
    pub fn add_from_stash(&mut self, agent: &str, ids: Vec<String>, cx: &mut Context<Self>) {
        self.keep_draft(agent, cx);
        let target = self.chat_target(agent);
        for id in ids {
            let asked = target.request("attachStash", json!({ "id": id, "agentId": agent }));
            cx.spawn(async move |bench, cx| {
                if let Err(error) = asked.await {
                    bench
                        .update(cx, |bench, cx| {
                            let words = format!("Couldn't add that from your Stash: {error}");
                            bench.notify(crate::toast::error(words), cx)
                        })
                        .ok();
                }
            })
            .detach();
        }
    }

    /// Keeps what is written in `agent`'s draft, and its files, as one prompt in the Stash, and
    /// empties the draft; the composer offers to put it back for a while.
    pub fn stash_prompt(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.keep_draft(agent, cx);
        let asked = self
            .chat_target(agent)
            .request("stashPrompt", json!({ "agentId": agent }));
        let agent = agent.to_owned();
        cx.spawn(async move |bench, cx| {
            let answer = asked.await;
            bench
                .update(cx, |bench, cx| match answer {
                    Ok(kept) => {
                        let id = kept["id"].as_str().map(str::to_owned);
                        if let Some(composing) = bench
                            .chats
                            .open
                            .get_mut(&agent)
                            .and_then(|chat| chat.composer.as_mut())
                        {
                            composing.stashed = id.clone();
                        }
                        cx.notify();
                        let agent = agent.clone();
                        cx.spawn(async move |bench, cx| {
                            cx.background_executor().timer(UNDO_FOR).await;
                            bench
                                .update(cx, |bench, cx| {
                                    if let Some(composing) = bench
                                        .chats
                                        .open
                                        .get_mut(&agent)
                                        .and_then(|chat| chat.composer.as_mut())
                                        .filter(|composing| composing.stashed == id)
                                    {
                                        composing.stashed = None;
                                        cx.notify();
                                    }
                                })
                                .ok();
                        })
                        .detach();
                    }
                    Err(error) => {
                        let words = format!("Couldn't stash this prompt: {error}");
                        bench.notify(crate::toast::error(words), cx)
                    }
                })
                .ok();
        })
        .detach();
    }

    /// Puts the prompt just stashed from `agent`'s draft back, and takes it off the Stash.
    pub fn unstash_prompt(&mut self, agent: &str, id: &str, cx: &mut Context<Self>) {
        if let Some(composing) = self
            .chats
            .open
            .get_mut(agent)
            .and_then(|chat| chat.composer.as_mut())
        {
            composing.stashed = None;
        }
        let target = self.chat_target(agent);
        let back = target.request("attachStash", json!({ "id": id, "agentId": agent }));
        let removed = json!({ "id": id });
        cx.spawn(async move |bench, cx| {
            let answer = match back.await {
                Ok(_) => target.request("removeStash", removed).await,
                Err(error) => Err(error),
            };
            if let Err(error) = answer {
                bench
                    .update(cx, |bench, cx| {
                        let words = format!("Couldn't put the prompt back: {error}");
                        bench.notify(crate::toast::error(words), cx)
                    })
                    .ok();
            }
        })
        .detach();
        cx.notify();
    }

    /// Takes stash item `id` off the account, on every device.
    pub fn remove_from_stash(&self, id: &str, cx: &mut Context<Self>) {
        let asked = crate::core::request(&self.asks, "removeStash", json!({ "id": id }));
        cx.spawn(async move |bench, cx| {
            if let Err(error) = asked.await {
                bench
                    .update(cx, |bench, cx| {
                        let words = format!("Couldn't remove that from your Stash: {error}");
                        bench.notify(crate::toast::error(words), cx)
                    })
                    .ok();
            }
        })
        .detach();
    }

    /// Opens a link in the browser, copies text, and opens a file in the app the system picks
    /// once this host has kept it.
    pub fn open_stash_item(&self, item: &Value, cx: &mut Context<Self>) {
        match item["kind"].as_str() {
            Some("link") => {
                if let Some(url) = item["url"].as_str() {
                    cx.open_url(url);
                }
            }
            Some("text" | "prompt") => {
                let text = item["text"].as_str().unwrap_or_default().to_owned();
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                self.notify(crate::toast::success("Copied"), cx);
            }
            _ => {
                let Some(id) = item["id"].as_str() else {
                    return;
                };
                let asked = crate::core::request(&self.asks, "stashFile", json!({ "id": id }));
                cx.spawn(async move |bench, cx| match asked.await {
                    Ok(kept) => {
                        if let Some(path) = kept["path"].as_str() {
                            let path = std::path::PathBuf::from(path);
                            cx.update(|cx| cx.open_with_system(&path));
                        }
                    }
                    Err(error) => {
                        bench
                            .update(cx, |bench, cx| {
                                let words = format!("Couldn't open that: {error}");
                                bench.notify(crate::toast::error(words), cx)
                            })
                            .ok();
                    }
                })
                .detach();
            }
        }
    }

    /// The person has seen everything in the stash, which every device then knows.
    pub fn see_stash(&mut self) {
        let Some(at) = Stash::of(self).newest().map(str::to_owned) else {
            return;
        };
        self.seen_stash_at(&at);
        // The ask is sent at once; the future only waits for an answer nobody needs.
        drop(crate::core::request(
            &self.asks,
            "seeStash",
            json!({ "at": at }),
        ));
    }

    /// Opens the Stash's pane in the strip in front, and marks the stash seen.
    pub fn open_stash(&mut self) {
        if let Some(strip) = self.strip() {
            let params = json!({ "kind": PaneKind::Stash, "target": "stash" });
            self.call_in_front("openTab", strip.with(params));
        }
        self.see_stash();
    }

    /// Whether the Stash's pane is the one in front.
    pub fn stash_in_front(&self) -> bool {
        self.tab()
            .and_then(|tab| crate::tabs::first_view(&tab.pane))
            .is_some_and(|(kind, _)| kind == PaneKind::Stash)
    }
}

/// Docked on a chat's composer, opened by its Stash button or by `&`: the newest items first, one
/// row each, the new ones tagged, to attach with a click or the keys; everything new at once; the
/// whole Stash; and stashing the prompt being written.
pub fn tray(
    bench: &Workbench,
    agent: &str,
    tray: &crate::chat::Tray,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let stash = Stash::of(bench);
    let fresh: Vec<String> = tray
        .fresh
        .iter()
        .filter(|id| {
            stash
                .items
                .iter()
                .any(|item| item["id"].as_str() == Some(id.as_str()))
        })
        .cloned()
        .collect();
    let link = |id: &'static str, words: String| {
        div()
            .id(id)
            .cursor_pointer()
            .text_size(theme::ui_text(TEXT_XS))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.text_secondary)
            .hover(move |link| link.text_color(t.text_primary))
            .child(words)
    };
    let add_new = (!fresh.is_empty() && tray.needle.is_empty()).then(|| {
        let (agent, ids) = (agent.to_owned(), fresh.clone());
        link("stash-tray-add-new", format!("Add {} new", ids.len())).on_click(cx.listener(
            move |bench, _, _, cx| {
                bench.add_from_stash(&agent, ids.clone(), cx);
                bench.dismiss_menu(&agent, cx);
            },
        ))
    });
    let open = {
        let agent = agent.to_owned();
        link("stash-tray-open", "Open Stash ↗".to_owned()).on_click(cx.listener(
            move |bench, _, _, cx| {
                bench.dismiss_menu(&agent, cx);
                bench.open_stash();
            },
        ))
    };
    let head = div()
        .px(SPACE_1H)
        .pb(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .child(ui::icon(Icon::Stack, 13.).text_color(t.text_tertiary))
        .child(
            ui::text("STASH", TEXT_3XS_H, FontWeight::SEMIBOLD)
                .letter_spacing(0.1)
                .text_color(t.text_tertiary),
        )
        .when(!tray.items.is_empty(), |head| {
            head.child(
                ui::text(tray.items.len().to_string(), TEXT_3XS_H, FontWeight::NORMAL)
                    .text_color(t.text_tertiary),
            )
        })
        .when(!tray.needle.is_empty(), |head| {
            head.child(
                ui::text(
                    format!("matching “{}”", tray.needle),
                    TEXT_2XS_H,
                    FontWeight::NORMAL,
                )
                .text_color(t.text_tertiary),
            )
        })
        .child(div().flex_1())
        .children(add_new)
        .child(open);
    let body = if tray.items.is_empty() {
        let words = match (stash.signed_in, tray.needle.is_empty()) {
            (false, _) => "Sign in to Sikemux to keep what you share from your phone here.",
            (true, true) => "Nothing in your Stash yet. Share a link, a picture or a file to Sikemux from any app on your phone.",
            (true, false) => "Nothing in your Stash matches.",
        };
        div()
            .py(SPACE_3H)
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(t.text_tertiary)
            .text_center()
            .child(words)
            .into_any_element()
    } else {
        let agents = crate::chat::named_agents(bench);
        let rows = tray.items.iter().enumerate().map(|(index, item)| {
            let new = item["id"]
                .as_str()
                .is_some_and(|id| fresh.iter().any(|fresh| fresh == id));
            let picked = index == tray.selected;
            tray_row(bench, agent, index, item, picked, new, &agents, cx)
        });
        div()
            .id("stash-tray-rows")
            .track_scroll(&tray.scroll)
            .max_h(px(TRAY_TALL))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(SPACE_0)
            .children(rows)
            .into_any_element()
    };
    let stash_prompt = {
        let agent = agent.to_owned();
        div()
            .id("stash-tray-prompt")
            .mt(SPACE_2)
            .mx(SPACE_1H)
            .pt(px(9.))
            .border_t_1()
            .border_color(t.line)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .cursor_pointer()
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(t.text_secondary)
            .hover(move |row| row.text_color(t.text_primary))
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.dismiss_menu(&agent, cx);
                bench.stash_prompt(&agent, cx);
            }))
            .child(ui::icon(Icon::Download, 13.).text_color(t.text_tertiary))
            .child("Stash this prompt")
            .child(div().flex_1())
            .child(ui::kbd("⌘S", cx))
    };
    div()
        .px(SPACE_2)
        .py(SPACE_3)
        .flex()
        .flex_col()
        .child(head)
        .child(body)
        .child(stash_prompt)
        .into_any_element()
}

/// A page an item links to, what the server read of it, and for a pull request or issue the
/// state the core knows it in.
#[derive(Clone, Copy)]
struct Page<'a> {
    url: &'a str,
    preview: &'a Value,
    state: Option<&'a str>,
}

impl Page<'_> {
    fn title(&self) -> String {
        (self.preview["title"].as_str().map(str::trim))
            .filter(|title| !title.is_empty())
            .map_or_else(|| bare_url(self.url), str::to_owned)
    }

    fn site(&self) -> String {
        (self.preview["site"].as_str().map(str::trim))
            .filter(|site| !site.is_empty())
            .map(str::to_owned)
            .or_else(|| site_of(self.url))
            .unwrap_or_else(|| bare_url(self.url))
    }
}

fn bare_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .to_owned()
}

/// A word of an item's text, or something it names, drawn as the chat draws its chip; a pull
/// request or issue in the colour of its state, when the core knows it.
enum Piece {
    Word(String),
    Pill {
        id: String,
        label: String,
        state: Option<String>,
    },
}

/// What a row shows of an item: its words, and what it carries besides them.
struct Said<'a> {
    words: Vec<Piece>,
    /// The page shown under the words, unless photos come first.
    lead: Option<Page<'a>>,
    /// The other pages, not to a pull request or issue, which the words name.
    pages: Vec<Page<'a>>,
    photos: Vec<&'a Value>,
    /// The files the words do not name.
    files: Vec<&'a Value>,
    /// The first agent the words name.
    agent: Option<String>,
}

fn said<'a>(item: &'a Value, agents: &[(String, String)]) -> Said<'a> {
    let attachments: Vec<&Value> = item["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .collect();
    let pages: Vec<Page> = match item["kind"].as_str() {
        Some("link") => (item["url"].as_str())
            .map(|url| Page {
                url,
                preview: &item["preview"],
                state: None,
            })
            .into_iter()
            .collect(),
        _ => (item["links"].as_array().into_iter().flatten())
            .filter_map(|link| {
                Some(Page {
                    url: link["url"].as_str()?,
                    preview: &link["preview"],
                    state: link["state"].as_str(),
                })
            })
            .collect(),
    };
    let text = item["words"].as_str().unwrap_or_default();
    let names: Vec<String> = (attachments.iter())
        .filter_map(|file| file["name"].as_str().map(str::to_owned))
        .collect();
    let context: Vec<SentContext> = (pages.iter())
        .map(|page| SentContext {
            uri: page.url.to_owned(),
            title: String::new(),
        })
        .collect();
    let named = crate::chat::named(text, &names, &context, agents, &[]);
    let words = pieces(text, &named, &pages);
    let (photos, files): (Vec<&Value>, Vec<&Value>) =
        (attachments.into_iter()).partition(|file| file["kind"].as_str() == Some("image"));
    let files = (files.into_iter())
        .filter(|file| {
            !(named.iter()).any(|mention| file["name"].as_str() == Some(mention.carried.as_str()))
        })
        .collect();
    let mut pages: Vec<Page> = (pages.into_iter())
        .filter(|page| sikemux_markdown::reference(page.url).is_none())
        .collect();
    let lead = (photos.is_empty() && !pages.is_empty()).then(|| pages.remove(0));
    let agent =
        (named.iter()).find_map(|mention| mention.id.strip_prefix("agent:").map(str::to_owned));
    Said {
        words,
        lead,
        pages,
        photos,
        files,
        agent,
    }
}

/// The words the core made of an item's text, with what `named` names in them as pills. The
/// core leaves in only links to pull requests and issues: one a mention names goes, and any
/// other reads as GitHub writes it.
fn pieces(text: &str, named: &[crate::chat::Mention], pages: &[Page]) -> Vec<Piece> {
    let state_of = |url: &str| {
        (pages.iter())
            .find(|page| page.url == url)
            .and_then(|page| page.state.map(str::to_owned))
    };
    let mut marks: Vec<(usize, usize, Option<Piece>)> = Vec::new();
    for mention in named {
        let id = match mention.id.strip_prefix("image:") {
            Some(name) => format!("file:{name}"),
            None => mention.id.clone(),
        };
        let pill = || Piece::Pill {
            id: id.clone(),
            label: mention.label.clone(),
            state: state_of(&mention.carried),
        };
        let found = text.match_indices(mention.written.as_str());
        marks.extend(found.map(|(at, written)| (at, at + written.len(), Some(pill()))));
    }
    for page in pages {
        let carried = named.iter().any(|mention| mention.carried == page.url);
        let pill = || {
            let reference = sikemux_markdown::reference(page.url).filter(|_| !carried)?;
            let kind = match page.url.contains("/pull") || reference.contains('!') {
                true => "pull",
                false => "issue",
            };
            Some(Piece::Pill {
                id: format!("tracked:{kind}:{}", page.url),
                label: reference,
                state: page.state.map(str::to_owned),
            })
        };
        let found = text.match_indices(page.url);
        marks.extend(found.map(|(at, url)| (at, at + url.len(), pill())));
    }
    marks.sort_by_key(|(at, end, _)| (*at, std::cmp::Reverse(*end)));
    let words = |text: &str| {
        text.split_whitespace()
            .map(|word| Piece::Word(word.to_owned()))
            .collect::<Vec<_>>()
    };
    let mut pieces = Vec::new();
    let mut from = 0;
    for (at, end, pill) in marks {
        if at < from || pieces.len() >= WORDS_LAID {
            continue;
        }
        pieces.extend(words(&text[from..at]));
        pieces.extend(pill);
        from = end;
    }
    pieces.extend(words(&text[from..]));
    pieces.truncate(WORDS_LAID);
    pieces
}

/// One item in the tray: a square with its picture or mark, its words over what it carries, and
/// how long ago it was stashed.
#[allow(clippy::too_many_arguments)]
fn tray_row(
    bench: &Workbench,
    agent: &str,
    index: usize,
    item: &Value,
    selected: bool,
    new: bool,
    agents: &[(String, String)],
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let said = said(item, agents);
    let words = match (said.words.is_empty(), said.lead) {
        (false, _) => sentence(item["id"].as_str(), &said.words, bench, &t, cx),
        (true, Some(page)) => sentence(None, &[Piece::Word(page.title())], bench, &t, cx),
        (true, None) => sentence(None, &[Piece::Word(name_of(item, &said))], bench, &t, cx),
    };
    let under = match (said.lead, item["kind"].as_str()) {
        (Some(page), _) => {
            let said_more = !said.words.is_empty();
            Some(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .min_w_0()
                    .child(favicon_small(bench, page, &t))
                    .child(
                        ui::text(
                            if said_more { page.title() } else { page.site() },
                            TEXT_2XS_H,
                            FontWeight::NORMAL,
                        )
                        .text_color(t.text_tertiary)
                        .truncate(),
                    ),
            )
        }
        (None, Some("file" | "video")) => item["size"].as_u64().map(|size| {
            div().child(
                ui::text(crate::codehost::bytes(size), TEXT_2XS_H, FontWeight::NORMAL)
                    .text_color(t.text_tertiary),
            )
        }),
        _ => None,
    };
    let files = said.files.iter().map(|file| file_chip(file, &t));
    let pages = (!said.pages.is_empty()).then(|| pages_chip(bench, &said, &t));
    let chips = (!said.files.is_empty() || pages.is_some()).then(|| {
        div()
            .pt(SPACE_1)
            .flex()
            .flex_wrap()
            .gap(px(5.))
            .children(files)
            .children(pages)
    });
    let ago = item["createdAt"].as_str().map(crate::ui::time::ago_written);
    let agent = agent.to_owned();
    div()
        .id(("stash-tray-row", index))
        .flex()
        .items_start()
        .gap(SPACE_3)
        .p(SPACE_2)
        .rounded(theme::RADIUS_2)
        .cursor_pointer()
        .when(selected, |row| row.bg(t.surface_chosen))
        .hover(move |row| row.bg(t.surface_chosen))
        .on_click(cx.listener(move |bench, _, window, cx| {
            bench.take_choice(&agent, Some(index), window, cx)
        }))
        .child(square(bench, item, &said, &t, cx))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(words)
                .children(under)
                .children(chips),
        )
        .child(
            div()
                .flex_none()
                .pt(SPACE_0)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .when(new, |aside| {
                    aside.child(
                        div()
                            .px(px(5.))
                            .rounded(theme::RADIUS_1)
                            .bg(t.accent)
                            .text_size(theme::ui_text(9.))
                            .font_weight(FontWeight::BOLD)
                            .letter_spacing(0.08)
                            .text_color(t.on_accent)
                            .child("NEW"),
                    )
                })
                .children(ago.map(|ago| {
                    ui::text(ago, TEXT_2XS, FontWeight::NORMAL).text_color(t.text_tertiary)
                })),
        )
        .into_any_element()
}

/// What to call an item with no words of its own: its file's name, or how many photos it holds.
fn name_of(item: &Value, said: &Said) -> String {
    match (said.photos.len(), said.files.first()) {
        (0, Some(file)) => file["name"].as_str().unwrap_or_default().to_owned(),
        (0, None) => title(item),
        (1, _) => "1 photo".to_owned(),
        (photos, _) => format!("{photos} photos"),
    }
}

/// Where a row's words end early, on a count of the pills that did not fit in two lines, and how
/// wide the words were when it was worked out, by the item's id.
#[derive(Clone, Copy)]
struct Cut {
    kept: usize,
    hidden: usize,
    width: Pixels,
}

#[derive(Default)]
struct Cuts(std::collections::HashMap<String, Cut>);

impl gpui_kit::Global for Cuts {}

/// How wide the count that ends cut words is, at most.
const MORE: f32 = 32.;

/// Up to two lines of words, with what they name as the chat's chips. Words of the item `key` whose
/// pills run past the two lines end on a count of them, worked out once they are laid out.
fn sentence(
    key: Option<&str>,
    pieces: &[Piece],
    bench: &Workbench,
    t: &Tokens,
    cx: &App,
) -> AnyElement {
    let cut = key.and_then(|key| cx.try_global::<Cuts>()?.0.get(key).copied());
    let shown = &pieces[..cut.map_or(pieces.len(), |cut| cut.kept.min(pieces.len()))];
    let space = px(3.5);
    let pill_line = |pill: Div| {
        pill.h(theme::ui_text(LINE - 3.))
            .max_w(px(PILL_WIDEST))
            .text_size(theme::ui_text(TEXT_XS))
    };
    let laid = shown.iter().enumerate().map(|(at, piece)| {
        let held = matches!(
            shown.get(at + 1),
            Some(Piece::Word(next)) if next.starts_with([',', '.', ';', ':', '!', '?', ')'])
        );
        let gap = if held { px(0.) } else { space };
        match piece {
            Piece::Word(word) => div().mr(gap).child(word.clone()).into_any_element(),
            Piece::Pill { id, label, state } => {
                let mark = state.as_deref().and_then(|state| state_mark(id, state, t));
                pill_line(crate::chat::pill(id, label, mark, bench, cx))
                    .mr(gap)
                    .into_any_element()
            }
        }
    });
    let more = cut.map(|cut| {
        pill_line(div())
            .px(px(5.))
            .flex()
            .items_center()
            .rounded(theme::RADIUS_2)
            .bg(t.surface_chosen)
            .text_color(t.text_secondary)
            .child(format!("+{}", cut.hidden))
    });
    let words = div()
        .flex()
        .flex_wrap()
        .items_center()
        .max_h(theme::ui_text(LINE * 2.))
        .overflow_hidden()
        .text_size(theme::ui_text(WORDS))
        .line_height(theme::ui_text(LINE))
        .text_color(t.text_primary)
        .children(laid)
        .children(more);
    let Some(key) = key.map(str::to_owned) else {
        return words.into_any_element();
    };
    let laid_out: Rc<Cell<Vec<Bounds<Pixels>>>> = Rc::default();
    let pills: Vec<bool> = (pieces.iter())
        .map(|piece| matches!(piece, Piece::Pill { .. }))
        .collect();
    let told = laid_out.clone();
    div()
        .w_full()
        .child(words.on_children_prepainted(move |bounds, _, _| told.set(bounds)))
        .on_children_prepainted(move |frame, window, cx| {
            let Some(frame) = frame.first() else {
                return;
            };
            let worked = cut_of(&laid_out.take(), &pills, frame, space);
            let fresh = match (cut, worked) {
                (Some(cut), _) if (cut.width - frame.size.width).abs() < px(0.5) => return,
                (Some(_), _) => None,
                (None, None) => return,
                (None, Some(worked)) => Some(worked),
            };
            let cuts = &mut cx.default_global::<Cuts>().0;
            match fresh {
                Some(fresh) => cuts.insert(key.clone(), fresh),
                None => cuts.remove(&key),
            };
            window.refresh();
        })
        .into_any_element()
}

/// Where words laid out in full in `frame` must end so every pill past their two lines is counted
/// at the end of the second: none when no pill falls past them.
fn cut_of(
    laid: &[Bounds<Pixels>],
    pills: &[bool],
    frame: &Bounds<Pixels>,
    space: Pixels,
) -> Option<Cut> {
    let mut lines = Vec::with_capacity(laid.len());
    let mut line = 0;
    for (at, bounds) in laid.iter().enumerate() {
        if at > 0 && bounds.origin.x < laid[at - 1].origin.x {
            line += 1;
        }
        lines.push(line);
    }
    let mut kept = lines.iter().take_while(|line| **line < 2).count();
    let hidden = |kept: usize| (pills.iter().skip(kept)).filter(|pill| **pill).count();
    if hidden(kept) == 0 {
        return None;
    }
    let right = frame.origin.x + frame.size.width;
    while kept > 0 && laid[kept - 1].origin.x + laid[kept - 1].size.width + space + px(MORE) > right
    {
        kept -= 1;
    }
    Some(Cut {
        kept,
        hidden: hidden(kept),
        width: frame.size.width,
    })
}

/// A pull request's or issue's glyph in the colour of `state`.
fn state_mark(id: &str, state: &str, t: &Tokens) -> Option<AnyElement> {
    let (glyph, ink, _) = match id.starts_with("tracked:pull") {
        true => crate::codehost::pull_look(serde_json::from_value(json!(state)).ok()?, t),
        false if state == "notPlanned" => {
            crate::codehost::issue_look("closed", Some("not_planned"), t)
        }
        false => crate::codehost::issue_look(state, None, t),
    };
    Some(ui::icon(glyph, 13.).text_color(ink).into_any_element())
}

/// The row's square: its first photo, counted when there are more; its own picture or its page's;
/// the mark of the first agent it names; or its glyph.
fn square(bench: &Workbench, item: &Value, said: &Said, t: &Tokens, cx: &App) -> AnyElement {
    let asks = bench.asks.clone();
    let shown = (said.photos.first())
        .and_then(|photo| photo["id"].as_str())
        .map(|id| Picture::Remote(Remote::stashed(id.to_owned(), asks.clone())))
        .or_else(|| picture(bench, item))
        .or_else(|| {
            let url = said.lead?.preview["imageUrl"].as_str()?;
            Some(Picture::Remote(Remote::new(url.to_owned(), asks)))
        });
    let named = said.agent.as_deref().and_then(|id| bench.agent_by_id(id));
    let mark = |cx: &App| match &named {
        Some(agent) => agent.mark(20., cx),
        None => glyph(item, 20., cx),
    };
    let tile = div()
        .size(px(THUMB))
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_2)
        .overflow_hidden()
        .bg(t.surface_raised);
    let tile = match shown {
        Some(shown) => {
            let fallback = std::cell::RefCell::new(Some(mark(cx)));
            tile.child(
                img(shown.source_at(px(THUMB * 2.)))
                    .size(px(THUMB))
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(move || {
                        fallback
                            .borrow_mut()
                            .take()
                            .unwrap_or_else(|| div().into_any_element())
                    }),
            )
        }
        None => tile.child(mark(cx)),
    };
    tile.when(said.photos.len() > 1, |tile| {
        tile.child(
            div()
                .absolute()
                .right(px(2.))
                .bottom(px(2.))
                .px(SPACE_1)
                .rounded(theme::RADIUS_1)
                .bg(gpui_kit::black().opacity(0.6))
                .text_size(theme::ui_text(9.5))
                .font_weight(FontWeight::BOLD)
                .text_color(gpui_kit::white())
                .child(said.photos.len().to_string()),
        )
    })
    .into_any_element()
}

fn chip(t: &Tokens) -> Div {
    div()
        .h(px(22.))
        .px(SPACE_1H)
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded(theme::RADIUS_1)
        .bg(t.surface_chosen)
        .text_size(theme::ui_text(TEXT_2XS_H))
        .text_color(t.text_secondary)
        .whitespace_nowrap()
}

/// A file the words do not name: its type, then its name.
fn file_chip(file: &Value, t: &Tokens) -> Div {
    let name = file["name"].as_str().unwrap_or("file");
    let (stem, kind) = name
        .rsplit_once('.')
        .filter(|(stem, kind)| !stem.is_empty() && (1..=4).contains(&kind.len()))
        .map_or((name, None), |(stem, kind)| {
            (stem, Some(kind.to_uppercase()))
        });
    chip(t)
        .pl(px(3.))
        .children(kind.map(|kind| {
            div()
                .h(px(16.))
                .px(SPACE_1)
                .flex()
                .items_center()
                .rounded(px(3.))
                .bg(t.surface_raised)
                .border_1()
                .border_color(t.line)
                .text_size(theme::ui_text(8.5))
                .font_weight(FontWeight::BOLD)
                .letter_spacing(0.04)
                .text_color(t.text_primary)
                .child(kind)
        }))
        .child(div().max_w(px(180.)).truncate().child(stem.to_owned()))
}

/// The pages beyond the one under the words: their sites' icons, stacked, and how many.
fn pages_chip(bench: &Workbench, said: &Said, t: &Tokens) -> Div {
    let count = said.pages.len();
    let words = match (said.lead.is_some(), count) {
        (true, 1) => "+1 link".to_owned(),
        (true, count) => format!("+{count} links"),
        (false, 1) => "1 link".to_owned(),
        (false, count) => format!("{count} links"),
    };
    let icons = said.pages.iter().take(3).enumerate().map(|(at, page)| {
        div()
            .when(at > 0, |icon| icon.ml(px(-4.)))
            .size(px(16.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .bg(t.surface_raised)
            .border_1()
            .border_color(t.line)
            .overflow_hidden()
            .child(favicon_small(bench, *page, t))
    });
    chip(t).child(div().flex().children(icons)).child(words)
}

/// A page's site icon, small: the one the page names, else the site's `favicon.ico`, else a globe.
fn favicon_small(bench: &Workbench, page: Page, t: &Tokens) -> AnyElement {
    let site = crate::chat::site_icon(page.url, true, Icon::Globe, Some(&bench.asks), t);
    let Some(named) = page.preview["iconUrl"].as_str() else {
        return site;
    };
    let shown = Picture::Remote(Remote::new(named.to_owned(), bench.asks.clone()));
    let fallback = std::cell::RefCell::new(Some(site));
    img(shown.source_at(px(14.)))
        .size(px(14.))
        .object_fit(ObjectFit::Contain)
        .with_fallback(move || {
            fallback
                .borrow_mut()
                .take()
                .unwrap_or_else(|| div().into_any_element())
        })
        .into_any_element()
}

/// A prompt's opening words over a row of its files' thumbnails.
fn prompt_preview(
    bench: &Workbench,
    item: &Value,
    inset: f32,
    size: f32,
    thumb: f32,
    t: &Tokens,
    cx: &App,
) -> AnyElement {
    let files = item["attachments"].as_array().cloned().unwrap_or_default();
    div()
        .size_full()
        .p(px(inset))
        .flex()
        .flex_col()
        .gap(SPACE_1H)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .text_size(theme::ui_text(size))
                .line_height(theme::ui_text(size * 1.4))
                .text_color(t.text_secondary)
                .child(item["text"].as_str().unwrap_or_default().trim().to_owned()),
        )
        .when(!files.is_empty(), |preview| {
            preview.child(
                div().flex_none().flex().gap(SPACE_1).children(
                    files
                        .iter()
                        .take(6)
                        .map(|file| mark(bench, file, thumb, cx)),
                ),
            )
        })
        .into_any_element()
}

/// The Stash's pane: every item as a card with its preview, to add to a chat, open or remove,
/// or drag onto a chat.
pub fn pane(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let stash = Stash::of(bench);
    let count = stash.items.len();
    let head = div()
        .flex_none()
        .px(SPACE_6)
        .pt(SPACE_5)
        .pb(SPACE_3H)
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .child(
            div()
                .flex()
                .items_center()
                .gap(SPACE_2)
                .child(ui::icon(Icon::Stack, 15.).text_color(t.text_secondary))
                .child(ui::text("Stash", 15., FontWeight::SEMIBOLD).text_color(t.text_primary))
                .when(stash.signed_in && count > 0, |head| {
                    head.child(
                        ui::text(count.to_string(), TEXT_XS, FontWeight::MEDIUM)
                            .text_color(t.text_tertiary),
                    )
                }),
        )
        .child(
            ui::text(
                "What you share to Sikemux from your phone. Add it to a chat, or drag it onto one.",
                TEXT_XS,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        );
    let body = if !stash.signed_in || stash.items.is_empty() {
        empty(stash.signed_in, &t).into_any_element()
    } else {
        let columns = cx.try_global::<Columns>().map_or(2, |columns| columns.0);
        let cards: Vec<(AnyElement, f32)> = stash
            .items
            .iter()
            .enumerate()
            .map(|(at, item)| (card(bench, at, item, cx), tall(item)))
            .collect();
        div()
            .id("stash-cards")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px(SPACE_6)
            .pb(SPACE_6)
            .child(grid(cards, columns))
            .into_any_element()
    };
    ui::pane(cx)
        .child(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(head)
                .child(div().h(px(1.)).mx(SPACE_6).mb(SPACE_4).bg(t.line))
                .child(body),
        )
        .into_any_element()
}

/// How many columns the stash's grid had room for when it was last drawn.
#[derive(Clone, Copy, PartialEq)]
struct Columns(u16);

impl gpui_kit::Global for Columns {}

/// Roughly how tall a card draws, for placing it in the shortest column: a compact link's is a
/// row, any other's has a picture.
fn tall(item: &Value) -> f32 {
    let compact = item["kind"].as_str() == Some("link") && item["preview"]["imageUrl"].is_null();
    let noted = item["note"]
        .as_str()
        .is_some_and(|note| !note.trim().is_empty());
    match (compact, noted) {
        (true, true) => 128.,
        (true, false) => 106.,
        (false, _) if matches!(item["kind"].as_str(), Some("text" | "prompt")) => 352.,
        (false, _) => 372.,
    }
}

/// The cards in as many columns as fit, each column as wide as the others and each card going
/// under the shortest column so far, so short cards stack rather than leave gaps.
fn grid(cards: Vec<(AnyElement, f32)>, columns: u16) -> impl IntoElement {
    let mut stacks: Vec<(f32, Vec<AnyElement>)> =
        (0..columns.max(1)).map(|_| (0., Vec::new())).collect();
    for (card, tall) in cards {
        if let Some(shortest) = stacks.iter_mut().min_by(|a, b| a.0.total_cmp(&b.0)) {
            shortest.0 += tall + GAP;
            shortest.1.push(card);
        }
    }
    div()
        .w_full()
        .on_children_prepainted(|bounds, window, cx| {
            let Some(width) = bounds.first().map(|bounds| f32::from(bounds.size.width)) else {
                return;
            };
            let fit = Columns(((width + GAP) / (MIN_CARD + GAP)).floor().max(1.) as u16);
            if cx.try_global::<Columns>() != Some(&fit) {
                cx.set_global(fit);
                window.refresh();
            }
        })
        .child(
            div()
                .w_full()
                .flex()
                .items_start()
                .gap(px(GAP))
                .children(stacks.into_iter().map(|(_, cards)| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(GAP))
                        .children(cards)
                })),
        )
}

fn empty(signed_in: bool, t: &Tokens) -> Div {
    let (head, words) = match signed_in {
        true => (
            "Nothing in your Stash",
            "Share a link, a picture or a file to Sikemux from any app on your phone. It waits here until you add it to a chat.",
        ),
        false => (
            "Your Stash is on your account",
            "Sign in to Sikemux to keep what you share from your phone here.",
        ),
    };
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_1H)
        .pb(px(64.))
        .child(
            div()
                .size(px(40.))
                .mb(SPACE_1H)
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_3)
                .bg(t.surface_raised)
                .border_1()
                .border_color(t.line)
                .child(ui::icon(Icon::Stack, 18.).text_color(t.text_tertiary)),
        )
        .child(ui::text(head, TEXT_MD, FontWeight::MEDIUM).text_color(t.text_primary))
        .child(
            div()
                .max_w(px(340.))
                .text_size(theme::ui_text(TEXT_XS_H))
                .line_height(theme::ui_text(18.))
                .text_color(t.text_tertiary)
                .text_center()
                .child(words),
        )
}

/// One item: its preview over its title, where it is from and its note, then what to do with it.
fn card(bench: &Workbench, at: usize, item: &Value, cx: &mut Context<Workbench>) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let id = item["id"].as_str().unwrap_or_default().to_owned();
    let note = item["note"]
        .as_str()
        .map(str::trim)
        .filter(|note| !note.is_empty())
        .map(|note| format!("“{note}”"));
    let dragged = DraggedStash { id: id.clone() };
    let label: SharedString = title(item).into();
    let ghost = label.clone();
    let compact = item["kind"].as_str() == Some("link") && item["preview"]["imageUrl"].is_null();
    let words = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(SPACE_0)
        .when(
            !matches!(item["kind"].as_str(), Some("text" | "prompt")),
            |words| {
                words.child(
                    ui::text(label, TEXT_XS_H, FontWeight::MEDIUM)
                        .text_color(t.text_primary)
                        .truncate(),
                )
            },
        )
        .child(
            ui::text(place(item), TEXT_3XS_H, FontWeight::NORMAL)
                .text_color(t.text_tertiary)
                .truncate(),
        );
    let body = match compact {
        true => div()
            .px(SPACE_3)
            .pt(SPACE_3)
            .flex()
            .items_center()
            .gap(SPACE_2H)
            .child(favicon(bench, item, &t, cx))
            .child(words.children(note.map(|note| {
                ui::text(note, TEXT_2XS_H, FontWeight::NORMAL)
                    .pt(px(3.))
                    .text_color(t.text_secondary)
                    .truncate()
            }))),
        false => div().px(SPACE_3).pt(SPACE_2H).child(
            words.child(
                ui::text(note.unwrap_or_default(), TEXT_2XS_H, FontWeight::NORMAL)
                    .h(px(19.))
                    .pt(px(3.))
                    .text_color(t.text_secondary)
                    .truncate(),
            ),
        ),
    };
    div()
        .id(("stash-card", at))
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(theme::RADIUS_3)
        .overflow_hidden()
        .border_1()
        .border_color(t.line)
        .bg(t.surface_raised)
        .hover(move |card| card.border_color(t.border_strong))
        .on_drag(dragged, move |_: &DraggedStash, _: Point<Pixels>, _, cx| {
            crate::drag::ghost(ghost.clone(), cx)
        })
        .when(!compact, |card| card.child(preview(bench, item, &t, cx)))
        .child(body)
        .child(actions(bench, at, item, &id, cx))
        .into_any_element()
}

/// A link's site in a small tile: its icon when the site has one, else a globe.
fn favicon(bench: &Workbench, item: &Value, t: &Tokens, cx: &App) -> AnyElement {
    let tile = div()
        .size(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_2)
        .overflow_hidden()
        .bg(t.surface_sunken)
        .border_1()
        .border_color(t.line);
    let icon = item["url"].as_str().and_then(|url| {
        let url = url::Url::parse(url).ok()?;
        Some(format!(
            "{}://{}/favicon.ico",
            url.scheme(),
            url.host_str()?
        ))
    });
    match icon {
        Some(icon) => {
            let fallback = std::cell::RefCell::new(Some(glyph(item, 15., cx)));
            let shown = Picture::Remote(Remote::new(icon, bench.asks.clone()));
            tile.child(
                img(shown.source_at(px(18.)))
                    .size(px(18.))
                    .object_fit(ObjectFit::Contain)
                    .with_fallback(move || {
                        fallback
                            .borrow_mut()
                            .take()
                            .unwrap_or_else(|| div().into_any_element())
                    }),
            )
            .into_any_element()
        }
        None => tile.child(glyph(item, 15., cx)).into_any_element(),
    }
}

/// The top of a card: a photo or a page's picture filling it, a text's opening lines, or a
/// file's glyph.
fn preview(bench: &Workbench, item: &Value, t: &Tokens, cx: &App) -> AnyElement {
    let frame = div()
        .h(px(PREVIEW))
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .bg(t.surface_sunken)
        .border_b_1()
        .border_color(t.line);
    let plain = |cx: &App| {
        let site = match item["kind"].as_str() {
            Some("link") => item["url"].as_str().and_then(site_of),
            Some("file" | "video" | "image") => item["name"]
                .as_str()
                .and_then(|name| name.rsplit_once('.'))
                .map(|(_, extension)| extension.to_uppercase()),
            _ => None,
        };
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(SPACE_2)
            .child(glyph(item, 26., cx))
            .children(site.map(|site| {
                ui::text(site, TEXT_3XS_H, FontWeight::MEDIUM)
                    .letter_spacing(0.04)
                    .text_color(Tokens::get(cx).text_tertiary)
            }))
            .into_any_element()
    };
    if item["kind"].as_str() == Some("prompt") {
        return frame
            .child(prompt_preview(bench, item, 12., 12., 30., t, cx))
            .into_any_element();
    }
    if item["kind"].as_str() == Some("text") {
        let text = item["text"].as_str().unwrap_or_default().trim().to_owned();
        return frame
            .items_start()
            .justify_start()
            .p(SPACE_3)
            .child(
                div()
                    .h_full()
                    .overflow_hidden()
                    .text_size(theme::ui_text(TEXT_XS))
                    .line_height(theme::ui_text(17.))
                    .text_color(t.text_secondary)
                    .child(text),
            )
            .into_any_element();
    }
    match picture(bench, item) {
        Some(shown) => {
            let fallback = std::cell::RefCell::new(Some(plain(cx)));
            frame
                .child(
                    img(shown.source_at(px(PICTURE)))
                        .w_full()
                        .h(px(PREVIEW))
                        .object_fit(ObjectFit::Cover)
                        .with_fallback(move || {
                            fallback
                                .borrow_mut()
                                .take()
                                .unwrap_or_else(|| div().into_any_element())
                        }),
                )
                .into_any_element()
        }
        None => frame.child(plain(cx)).into_any_element(),
    }
}

/// Add to chat, into the chat in front or one picked from the open chats; Open; Remove.
fn actions(
    bench: &Workbench,
    at: usize,
    item: &Value,
    id: &str,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let agents: Vec<(String, String)> = bench.shown().map_or_else(Vec::new, |view| {
        view.workspace
            .agents
            .iter()
            .map(|agent| {
                let name = agent.name.clone().unwrap_or_else(|| agent.title.clone());
                (agent.id.clone(), name)
            })
            .collect()
    });
    let focused = bench
        .focused_agent()
        .and_then(|focused| agents.iter().find(|(agent, _)| agent == focused))
        .cloned();
    let this = cx.entity().downgrade();
    let chooser = move |agents: Vec<(String, String)>, id: String| {
        let this = this.clone();
        move |mut menu: gpui_kit::component::menu::PopupMenu,
              _: &mut Window,
              _: &mut Context<gpui_kit::component::menu::PopupMenu>| {
            for (agent, name) in &agents {
                let (agent, id, this) = (agent.clone(), id.clone(), this.clone());
                menu = menu.item(
                    gpui_kit::component::menu::PopupMenuItem::new(name.clone()).on_click(
                        move |_, _, cx| {
                            this.update(cx, |bench, cx| {
                                bench.add_from_stash(&agent, vec![id.clone()], cx)
                            })
                            .ok();
                        },
                    ),
                );
            }
            menu
        }
    };
    let button =
        || ui::Button::new(("stash-add", at), "Add to chat", ui::Variant::Outline).icon(Icon::Plus);
    let add = match focused {
        Some((agent, name)) => {
            let others: Vec<(String, String)> = agents
                .iter()
                .filter(|(other, _)| *other != agent)
                .cloned()
                .collect();
            let picker = (!others.is_empty()).then(|| {
                ui::menu_button(
                    ("stash-pick", at),
                    gpui_kit::Anchor::TopLeft,
                    IconButton::new(
                        ("stash-pick-face", at),
                        Icon::Chevron,
                        "Add to another chat",
                    )
                    .small()
                    .turned(std::f32::consts::FRAC_PI_2),
                    chooser(others, id.to_owned()),
                )
            });
            let id = id.to_owned();
            div()
                .flex()
                .items_center()
                .gap(SPACE_0)
                .child(
                    button()
                        .tooltip(format!("Add to {name}"))
                        .on_click(cx.listener(move |bench, _, _, cx| {
                            bench.add_from_stash(&agent, vec![id.clone()], cx)
                        })),
                )
                .children(picker)
                .into_any_element()
        }
        None if !agents.is_empty() => ui::menu_button(
            ("stash-choose", at),
            gpui_kit::Anchor::TopLeft,
            button(),
            chooser(agents, id.to_owned()),
        )
        .into_any_element(),
        None => button()
            .disabled(true)
            .tooltip("Open a chat to add this to it")
            .into_any_element(),
    };
    let (opened, removed) = (item.clone(), id.to_owned());
    let open_label = match item["kind"].as_str() {
        Some("link") => "Open in browser",
        Some("text" | "prompt") => "Copy",
        _ => "Open",
    };
    let open_icon = match item["kind"].as_str() {
        Some("text" | "prompt") => Icon::Copy,
        Some("link") => Icon::Globe,
        _ => Icon::Eye,
    };
    div()
        .px(SPACE_2)
        .pt(SPACE_2H)
        .pb(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_0)
        .child(add)
        .child(div().flex_1())
        .child(
            IconButton::new(("stash-open", at), open_icon, open_label)
                .small()
                .on_click(cx.listener(move |bench, _, _, cx| bench.open_stash_item(&opened, cx))),
        )
        .child(
            IconButton::new(("stash-remove", at), Icon::Trash, "Remove from Stash")
                .small()
                .on_click(
                    cx.listener(move |bench, _, _, cx| bench.remove_from_stash(&removed, cx)),
                ),
        )
}

/// Takes stash items dropped on `agent`'s chat into its draft.
pub fn takes_drops(pane: Div, agent: &str, cx: &mut Context<Workbench>) -> Div {
    let (agent, this) = (agent.to_owned(), cx.weak_entity());
    pane.on_drop(move |dropped: &DraggedStash, _: &mut Window, cx| {
        this.update(cx, |bench, cx| {
            bench.add_from_stash(&agent, vec![dropped.id.clone()], cx)
        })
        .ok();
    })
}
