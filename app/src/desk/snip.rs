//! Snip mode on a desk's pages: each box the person drags over the page in front goes to the
//! project's front chat as a picture at its draft's caret, with what the page holds there.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{Context, Window};
use serde_json::Value;
use sikemux_browser::snip::{self, Snip, Snipped};
use sikemux_browser::BrowserManager;
use tokio::sync::{mpsc, oneshot, Notify};

use super::{Desks, Heard};
use crate::chat::SNIP_NAME;
use crate::send_to::{self, Recipient};
use crate::theme::Tokens;
use crate::toast;
use crate::workbench::Workbench;

/// How long a desk in snip mode waits before trying its page again after the page could not
/// take a box, as while it loads.
const RETRY: Duration = Duration::from_secs(1);

/// One desk's snip mode, which ends when this is dropped.
pub(crate) struct Snipper {
    on: Arc<AtomicBool>,
    poke: Arc<Notify>,
}

impl Drop for Snipper {
    fn drop(&mut self) {
        self.on.store(false, Ordering::Relaxed);
        self.poke.notify_one();
    }
}

impl Desks {
    pub(crate) fn snipping(&self, agent: &str) -> bool {
        self.snips.contains_key(agent)
    }

    fn toggle_snipping(&mut self, agent: &str, cx: &mut Context<Self>) {
        if self.snips.remove(agent).is_none() {
            let Some(engine) = self.engine.clone() else {
                return;
            };
            let snipper = Snipper {
                on: Arc::new(AtomicBool::new(true)),
                poke: Arc::new(Notify::new()),
            };
            let accent = Tokens::get(cx).accent.to_rgb();
            let accent = format!(
                "#{:02x}{:02x}{:02x}",
                (accent.r * 255.).round() as u8,
                (accent.g * 255.).round() as u8,
                (accent.b * 255.).round() as u8
            );
            super::spawn(waiting(
                engine,
                agent.to_owned(),
                accent,
                snipper.on.clone(),
                snipper.poke.clone(),
                self.told.clone(),
            ));
            self.snips.insert(agent.to_owned(), snipper);
        }
        cx.notify();
    }

    /// Has each desk in snip mode put its page in front back in it, after its tabs changed.
    pub(crate) fn rearm_snips(&self) {
        for snipper in self.snips.values() {
            snipper.poke.notify_one();
        }
    }
}

/// Keeps the page in front of `agent`'s desk waiting for boxes while `on`, and tells the window
/// each one. A poke starts the wait again on whatever page is in front then.
async fn waiting(
    engine: Arc<BrowserManager>,
    agent: String,
    accent: String,
    on: Arc<AtomicBool>,
    poke: Arc<Notify>,
    told: mpsc::UnboundedSender<Heard>,
) {
    let mut armed: Option<String> = None;
    let mut taken: Option<Value> = None;
    while on.load(Ordering::Relaxed) {
        let shown = engine
            .snapshot(&agent)
            .ok()
            .and_then(|strip| strip.active_tab_id);
        if let Some(left) = armed.take().filter(|tab| Some(tab) != shown.as_ref()) {
            snip::stop(&engine, &agent, &left).await;
            taken = None;
        }
        armed = shown;
        let flash = taken.take();
        let waited = tokio::select! {
            waited = snip::wait(&engine, &agent, flash.as_ref(), &accent) => Some(waited),
            () = poke.notified() => None,
        };
        match waited {
            Some(Ok(Snipped::Boxed { tab_id, page })) => {
                if let Ok(snip) = snip::take(&engine, &agent, tab_id, page).await {
                    taken = Some(snip.area());
                    let _ = told.send(Heard::Snipped(snip));
                }
            }
            Some(Ok(Snipped::Pinned { pin })) => {
                let _ = told.send(Heard::Pinned(agent.clone(), pin));
            }
            Some(Ok(Snipped::Ended)) => {
                let _ = told.send(Heard::SnipEnded(agent));
                return;
            }
            Some(Ok(Snipped::Nothing)) | None => {}
            Some(Err(_)) => {
                let _ = tokio::time::timeout(RETRY, poke.notified()).await;
            }
        }
    }
    if let Some(tab) = armed {
        snip::stop(&engine, &agent, &tab).await;
    }
}

impl Workbench {
    pub(crate) fn toggle_snip(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.desks
            .update(cx, |desks, cx| desks.toggle_snipping(agent, cx));
    }

    /// Snip mode on the front agent's pages, on or off.
    pub fn toggle_front_snip(&mut self, cx: &mut Context<Self>) {
        match self.front_pages(cx) {
            Some(agent) => self.toggle_snip(&agent, cx),
            None => self.notify(toast::info("Open a page on the desk to snip it"), cx),
        }
    }

    pub(crate) fn snip_ended(&mut self, agent: &str, cx: &mut Context<Self>) {
        self.desks.update(cx, |desks, cx| {
            desks.snips.remove(agent);
            cx.notify();
        });
    }

    /// Hands a box the person took to the project's front chat, starting one when none is open.
    pub(crate) fn snipped(&mut self, snip: Snip, window: &mut Window, cx: &mut Context<Self>) {
        let name = format!(
            "{SNIP_NAME}{}.jpg",
            chrono::Local::now().format("%Y%m%d-%H%M%S%3f")
        );
        let held = self
            .desks
            .update(cx, |desks, _| match &mut desks.snips_for_new_chat {
                Some(waiting) => {
                    waiting.push((name, snip));
                    None
                }
                None => Some((name, snip)),
            });
        let Some((name, snip)) = held else {
            return;
        };
        let Some(choice) = send_to::choices(self).into_iter().next() else {
            self.notify(
                toast::info("Start an agent first — snips go to its chat"),
                cx,
            );
            return;
        };
        let (launcher, project) = match choice.to {
            Recipient::Agent(agent) => return self.attach_snip(&agent, name, snip, window, cx),
            Recipient::NewChat { launcher, project } => (launcher, project),
        };
        self.desks.update(cx, |desks, _| {
            desks.snips_for_new_chat = Some(vec![(name, snip)]);
        });
        let (started, agent) = oneshot::channel();
        self.target()
            .start_chat(launcher, project, None, Some(started));
        cx.spawn_in(window, async move |bench, cx| {
            let agent = agent.await.ok();
            let _ = bench.update_in(cx, |bench, window, cx| {
                let waiting = bench
                    .desks
                    .update(cx, |desks, _| desks.snips_for_new_chat.take());
                let Some(agent) = agent else {
                    return;
                };
                for (name, snip) in waiting.into_iter().flatten() {
                    bench.attach_snip(&agent, name, snip, window, cx);
                }
            });
        })
        .detach();
    }

    fn attach_snip(
        &mut self,
        agent: &str,
        name: String,
        snip: Snip,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = snip.page;
        self.attach_picture(
            agent,
            name,
            "image/jpeg",
            snip.jpeg,
            move |path| about(path, &page),
            window,
            cx,
        );
    }
}

/// What the agent alone is told about the snip kept at `path`, as a title and text: the page it
/// came from, where the box sat, and the elements in it.
fn about(path: &Path, page: &Value) -> (String, String) {
    let file = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    let text_of = |key: &str| page[key].as_str().unwrap_or_default();
    let number = |value: &Value| value.as_f64().unwrap_or(0.0).round();
    let mut text = format!(
        "{file} is the part of {} (\"{}\") I boxed: {} × {} at {}, {} in its {} × {} view.",
        text_of("url"),
        text_of("title"),
        number(&page["width"]),
        number(&page["height"]),
        number(&page["left"]),
        number(&page["top"]),
        number(&page["viewport"]["width"]),
        number(&page["viewport"]["height"]),
    );
    let element = |text: &mut String, element: &Value| {
        let field = |key: &str| element[key].as_str().unwrap_or_default();
        text.push_str(&format!(
            "\n- <{}> `{}` \"{}\"\n  {}",
            field("tag"),
            field("selector"),
            field("text"),
            field("html")
        ));
    };
    if let Some(elements) = page["elements"].as_array() {
        text.push_str("\n\nElements in the box:");
        for each in elements {
            element(&mut text, each);
        }
        if let Some(more) = page["more"].as_u64() {
            text.push_str(&format!("\n- and {more} more"));
        }
    } else if page["holder"].is_object() {
        text.push_str("\n\nThe element holding the box:");
        element(&mut text, &page["holder"]);
    }
    (format!("Snip {file}"), text)
}
