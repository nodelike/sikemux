//! Pins on a desk's pages: a click in snip mode puts a note on one spot, which goes to the
//! desk's agent as the person's message and stays on the page, drawn by the page itself, until
//! the agent answers it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use gpui_kit::{Context, Hsla};
use serde_json::{json, Value};
use sikemux_browser::pins::{self, PinAnswer};
use sikemux_browser::viewport::Viewport;
use sikemux_browser::BrowserManager;
use sikemux_core::server::call;
use sikemux_view::pins::{
    Pin, PinAfterParams, PinBy, PinNoteParams, PinParams, PinState, PlacePinParams,
};
use tokio::sync::mpsc;

use super::Heard;
use crate::theme::Tokens;
use crate::toast;
use crate::workbench::Workbench;

/// What the desks keep of the pins they draw.
#[derive(Default)]
pub(crate) struct PinsShown {
    /// What each agent's page in front was last told, so a page is told again only on a change.
    told: HashMap<String, String>,
    /// The desks waiting for the person to answer a pin in the page, each ended by its flag.
    waits: HashMap<String, Arc<AtomicBool>>,
    /// Answered pins whose after picture is being taken.
    taking: HashSet<String>,
    /// Pictures read once as data the page can draw, by path.
    pictures: HashMap<String, String>,
}

/// One page's pin as the page draws it.
fn drawn(pin: &Pin, pictures: &mut HashMap<String, String>) -> Value {
    let who = pin
        .agent_name
        .as_ref()
        .map_or_else(|| "The agent".to_owned(), |name| format!("@{name}"));
    let notes: Vec<Value> = (pin.notes.iter())
        .map(|note| {
            let by = match note.by {
                PinBy::Agent => "agent",
                _ => "person",
            };
            json!({ "by": by, "words": note.words })
        })
        .collect();
    let mut data = |path: &Option<String>| {
        let path = path.as_ref()?;
        if !pictures.contains_key(path) {
            let bytes = std::fs::read(path).ok()?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
            pictures.insert(path.clone(), format!("data:image/jpeg;base64,{encoded}"));
        }
        pictures.get(path).cloned()
    };
    let answered = pin.state == PinState::Answered;
    json!({
        "id": pin.id,
        "selector": pin.selector,
        "state": if answered { "answered" } else { "open" },
        "reopened": pin.reopened,
        "working": pin.working,
        "since": pin.since,
        "who": who,
        "notes": notes,
        "beforeData": answered.then(|| data(&pin.before)).flatten(),
        "afterData": answered.then(|| data(&pin.after)).flatten(),
    })
}

fn hex(color: Hsla) -> String {
    let rgb = color.to_rgb();
    let channel = |value: f32| (value * 255.).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(rgb.r),
        channel(rgb.g),
        channel(rgb.b)
    )
}

/// Where the pictures of pins' spots are kept on this host.
fn picture_path() -> Option<PathBuf> {
    let folder = sikemux_process::paths::config_dir()?.join("pins");
    std::fs::create_dir_all(&folder).ok()?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let next = NEXT.fetch_add(1, Ordering::Relaxed);
    Some(folder.join(format!("{}-{next}.jpg", crate::ui::time::now_ms())))
}

/// A picture of the element `selector` finds on `url` at `width` × `height`, kept on this host.
async fn picture(
    engine: &BrowserManager,
    agent: &str,
    url: &str,
    (width, height): (u32, u32),
    selector: &str,
) -> Option<String> {
    let fixed = Viewport::sized(width, height).ok()?;
    let jpeg = engine.picture_of(agent, url, fixed, selector).await.ok()?;
    let path = picture_path()?;
    std::fs::write(&path, jpeg).ok()?;
    Some(path.to_string_lossy().into_owned())
}

/// Hears the person answer pins on the page in front of `agent`'s desk while `on`.
async fn waiting(
    engine: Arc<BrowserManager>,
    agent: String,
    on: Arc<AtomicBool>,
    told: mpsc::UnboundedSender<Heard>,
) {
    while on.load(Ordering::Relaxed) {
        match pins::wait(&engine, &agent).await {
            Ok(PinAnswer::Nothing) => {}
            Ok(answer) => {
                let _ = told.send(Heard::PinAnswered(answer));
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
        }
    }
}

impl Workbench {
    /// Draws each desk's pins over its page in front, and takes the after picture of each pin
    /// newly answered.
    pub(crate) fn show_pins(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let pins: Vec<Pin> = self
            .this_view()
            .map(|view| view.pins.clone())
            .unwrap_or_default();
        let t = Tokens::get(cx).clone();
        let desks: Vec<(String, Option<sikemux_browser::BrowserTab>)> =
            (self.desks.read(cx).strips.iter())
                .map(|(agent, strip)| {
                    let front = strip.tabs.iter().find(|tab| tab.active).cloned();
                    (agent.clone(), front)
                })
                .collect();
        for (agent, front) in desks {
            let Some(front) = front.filter(|front| !front.loading) else {
                self.desks
                    .update(cx, |desks, _| desks.pins_shown.told.remove(&agent));
                continue;
            };
            let to = self
                .agent_by_id(&agent)
                .and_then(|row| row.handle().map(|name| format!("@{name}")));
            let mine: Vec<&Pin> = (pins.iter())
                .filter(|pin| pin.agent_id == agent && pin.url == front.url)
                .collect();
            let told = self.desks.update(cx, |desks, _| {
                let shown = &mut desks.pins_shown;
                let list: Vec<Value> = mine
                    .iter()
                    .map(|pin| drawn(pin, &mut shown.pictures))
                    .collect();
                let look = pins::look(
                    &hex(t.accent),
                    &hex(t.surface_overlay),
                    &hex(t.text_primary),
                    &hex(t.text_tertiary),
                    &hex(t.border),
                    to.as_deref(),
                );
                let key = format!("{}|{}|{list:?}|{look}", front.id, front.url);
                (shown.told.get(&agent) != Some(&key)).then(|| {
                    shown.told.insert(agent.clone(), key);
                    (Value::Array(list), look)
                })
            });
            if let Some((list, look)) = told {
                let (engine, agent) = (engine.clone(), agent.clone());
                super::spawn(async move {
                    let _ = pins::show(&engine, &agent, &list, &look).await;
                });
            }
            self.keep_pin_wait(&agent, !mine.is_empty(), &engine, cx);
        }
        for pin in pins
            .iter()
            .filter(|pin| pin.state == PinState::Answered && pin.after.is_none())
        {
            self.take_after(pin, &engine, cx);
        }
    }

    fn keep_pin_wait(
        &mut self,
        agent: &str,
        wanted: bool,
        engine: &Arc<BrowserManager>,
        cx: &mut Context<Self>,
    ) {
        self.desks.update(cx, |desks, _| {
            let running = desks.pins_shown.waits.contains_key(agent);
            match (wanted, running) {
                (true, false) => {
                    let on = Arc::new(AtomicBool::new(true));
                    super::spawn(waiting(
                        engine.clone(),
                        agent.to_owned(),
                        on.clone(),
                        desks.told.clone(),
                    ));
                    desks.pins_shown.waits.insert(agent.to_owned(), on);
                }
                (false, true) => {
                    if let Some(on) = desks.pins_shown.waits.remove(agent) {
                        on.store(false, Ordering::Relaxed);
                    }
                }
                _ => {}
            }
        });
    }

    fn take_after(&mut self, pin: &Pin, engine: &Arc<BrowserManager>, cx: &mut Context<Self>) {
        let fresh = self.desks.update(cx, |desks, _| {
            desks.pins_shown.taking.insert(pin.id.clone())
        });
        if !fresh {
            return;
        }
        let (engine, pin) = (engine.clone(), pin.clone());
        let target = self.chat_target(&pin.agent_id);
        super::spawn(async move {
            let size = (pin.width, pin.height);
            if let Some(after) =
                picture(&engine, &pin.agent_id, &pin.url, size, &pin.selector).await
            {
                target.call_typed::<call::PinAfter>(PinAfterParams { id: pin.id, after });
            }
        });
    }

    /// The person clicked an element in snip mode and wrote a note: the spot's picture is taken
    /// and the pin goes to the desk's agent.
    pub(crate) fn pinned(&mut self, agent: &str, pin: Value, cx: &mut Context<Self>) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let text = |key: &str| pin[key].as_str().unwrap_or_default().to_owned();
        let size = |key: &str| pin["viewport"][key].as_f64().unwrap_or(0.).round() as u32;
        let mut params = PlacePinParams {
            agent_id: agent.to_owned(),
            url: text("url"),
            title: text("page"),
            width: size("width"),
            height: size("height"),
            label: text("label"),
            selector: text("selector"),
            html: text("html"),
            before: None,
            note: text("note"),
        };
        let to = self
            .agent_by_id(agent)
            .and_then(|row| row.handle().map(|name| format!("@{name}")))
            .unwrap_or_else(|| "the agent".to_owned());
        let target = self.chat_target(agent);
        let bench = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let taken = super::run({
                let (engine, agent, url, selector) = (
                    engine.clone(),
                    params.agent_id.clone(),
                    params.url.clone(),
                    params.selector.clone(),
                );
                let size = (params.width, params.height);
                async move { picture(&engine, &agent, &url, size, &selector).await }
            })
            .await
            .flatten();
            params.before = taken;
            let placed = target.request_typed::<call::PlacePin>(params).await;
            cx.update(|cx| {
                let note = match placed {
                    Ok(_) => toast::success(format!("Pin sent to {to}")),
                    Err(error) => toast::error(format!("The pin did not go: {error}")),
                };
                toast::show(&bench, note, cx);
            })
        })
        .detach();
    }

    /// The person answered a pin in the page's float.
    pub(crate) fn pin_answered(&mut self, answer: PinAnswer, cx: &mut Context<Self>) {
        let target = crate::core::Target::here(&self.asks);
        let bench = cx.weak_entity();
        type Asked = std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>;
        let (asked, done): (Asked, &str) = match answer {
            PinAnswer::Note { id, words } => (
                Box::pin(target.request_typed::<call::NotePin>(PinNoteParams { id, words })),
                "Sent back to the agent",
            ),
            PinAnswer::LooksRight { id } => (
                Box::pin(target.request_typed::<call::PinLooksRight>(PinParams { id })),
                "",
            ),
            PinAnswer::Nothing => return,
        };
        cx.spawn(async move |_, cx| {
            let said = match asked.await {
                Ok(()) if done.is_empty() => return,
                Ok(()) => toast::success(done),
                Err(error) => toast::error(error),
            };
            cx.update(|cx| toast::show(&bench, said, cx))
        })
        .detach();
    }
}
