//! Pins on a desk's pages: a click in snip mode puts a note on one spot, which goes to the
//! desk's agent and stays on the page, drawn by the page itself, until the agent answers it.
//! Pictures of the spot come only from the person's own tab while it shows the page, or for a
//! page this machine serves, from a private view off screen that sees nothing signed in.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use gpui_kit::{Context, Hsla};
use serde_json::{json, Value};
use sikemux_browser::pins::{self, PinAnswer};
use sikemux_browser::{BrowserManager, BrowserTab};
use sikemux_core::server::call;
use sikemux_view::pins::{
    Pin, PinAfterParams, PinBy, PinNoteParams, PinParams, PinPictureParams, PinState,
    PlacePinParams,
};
use tokio::sync::mpsc;

use super::Heard;
use crate::theme::Tokens;
use crate::toast;
use crate::workbench::Workbench;

/// How many pictures are kept as data beyond those a page in front shows; the least recently
/// used go first. A picture a page in front shows is never let go, or the two would take turns
/// pushing each other out.
const KEPT_PICTURES: usize = 16;

/// What the desks keep of the pins they draw.
#[derive(Default)]
pub(crate) struct PinsShown {
    /// What each agent's page in front was last told, so a page is told again only on a change.
    told: HashMap<String, String>,
    /// The desks waiting for the person to answer a pin in the page, each ended by its flag.
    waits: HashMap<String, Arc<AtomicBool>>,
    /// Answered pins whose after picture is being taken.
    taking: HashSet<String>,
    pictures: Pictures,
}

#[derive(Default)]
struct Pictures {
    data: HashMap<String, String>,
    order: VecDeque<String>,
    fetching: HashSet<String>,
    /// Pictures the core had no longer, asked for again only once a pin names them anew.
    missing: HashSet<String>,
}

impl Pictures {
    fn get(&mut self, id: &str) -> Option<String> {
        let data = self.data.get(id).cloned()?;
        self.order.retain(|kept| kept != id);
        self.order.push_back(id.to_owned());
        Some(data)
    }

    fn keep(&mut self, id: String, data: String) {
        self.fetching.remove(&id);
        self.order.retain(|kept| kept != &id);
        self.order.push_back(id.clone());
        self.data.insert(id, data);
    }

    /// Lets go of the least recently used pictures past the bound, keeping every one in `shown`.
    fn trim(&mut self, shown: &HashSet<String>) {
        let mut spare = self.order.len().saturating_sub(shown.len() + KEPT_PICTURES);
        let data = &mut self.data;
        self.order.retain(|id| {
            let drop = spare > 0 && !shown.contains(id);
            if drop {
                spare -= 1;
                data.remove(id);
            }
            !drop
        });
    }

    fn forget(&mut self, id: &str) {
        self.data.remove(id);
        self.order.retain(|kept| kept != id);
    }
}

/// The page a pin belongs to: its address without the query or the fragment.
fn page_of(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// A page this machine serves, which the private view off screen may load by itself.
fn served_here(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| {
        url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    })
}

/// What decides how a pin is drawn, so a page is told again only when one of these moves.
fn key_of(pin: &Pin, pictures: &Pictures) -> String {
    let has = |id: &Option<String>| id.as_ref().is_some_and(|id| pictures.data.contains_key(id));
    format!(
        "{}:{:?}:{}:{}:{}:{}:{}:{}:{}",
        pin.id,
        pin.state,
        pin.reopened,
        pin.working,
        pin.stopped,
        pin.since,
        pin.notes.len(),
        has(&pin.before),
        has(&pin.after),
    )
}

/// One page's pin as the page draws it.
fn drawn(pin: &Pin, pictures: &mut Pictures) -> Value {
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
    let answered = pin.state == PinState::Answered;
    let mut data = |id: &Option<String>| answered.then(|| pictures.get(id.as_deref()?)).flatten();
    json!({
        "id": pin.id,
        "selector": pin.selector,
        "state": if answered { "answered" } else { "open" },
        "reopened": pin.reopened,
        "working": pin.working,
        "stopped": pin.stopped,
        "since": pin.since,
        "who": who,
        "notes": notes,
        "beforeData": data(&pin.before),
        "afterData": data(&pin.after),
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

fn encoded(jpeg: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(jpeg)
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
    /// Draws each desk's pins over its page in front, fetching the pictures an answered pin
    /// shows, and takes the after picture of an answered pin its page in front shows.
    pub(crate) fn show_pins(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.desks.read(cx).engine.clone() else {
            return;
        };
        let Some(view) = self.this_view() else {
            return;
        };
        if view.pins.is_empty() && self.desks.read(cx).pins_shown.told.is_empty() {
            return;
        }
        let pins: Vec<Pin> = view.pins.clone();
        let t = Tokens::get(cx).clone();
        for pin in pins.iter().filter(|pin| {
            pin.state == PinState::Answered
                && pin.after.is_none()
                && !pin.after_by_hand
                && served_here(&pin.url)
        }) {
            self.take_after_privately(pin, &engine, cx);
        }
        let fronts: Vec<(String, Option<BrowserTab>)> = (self.desks.read(cx).strips.iter())
            .map(|(agent, strip)| {
                (
                    agent.clone(),
                    strip.tabs.iter().find(|tab| tab.active).cloned(),
                )
            })
            .collect();
        let mut in_sight = HashSet::new();
        for (agent, front) in fronts {
            let Some(front) = front.filter(|front| !front.loading) else {
                self.desks
                    .update(cx, |desks, _| desks.pins_shown.told.remove(&agent));
                continue;
            };
            let to = self
                .agent_by_id(&agent)
                .and_then(|row| row.handle().map(|name| format!("@{name}")));
            let mine: Vec<&Pin> = (pins.iter())
                .filter(|pin| pin.agent_id == agent && page_of(&pin.url) == page_of(&front.url))
                .collect();
            let wanted: Vec<String> = (mine.iter())
                .filter(|pin| pin.state == PinState::Answered)
                .flat_map(|pin| pin.before.iter().chain(&pin.after).cloned())
                .collect();
            for id in wanted {
                in_sight.insert(id.clone());
                self.fetch_pin_picture(id, cx);
            }
            let told = self.desks.update(cx, |desks, _| {
                let shown = &mut desks.pins_shown;
                let mut key = format!("{}|{}|{to:?}", front.id, page_of(&front.url));
                for pin in &mine {
                    key.push('|');
                    key.push_str(&key_of(pin, &shown.pictures));
                }
                if shown.told.get(&agent) == Some(&key) {
                    return None;
                }
                shown.told.insert(agent.clone(), key);
                let list: Vec<Value> = (mine.iter())
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
                Some((Value::Array(list), look))
            });
            if let Some((list, look)) = told {
                let (engine, agent) = (engine.clone(), agent.clone());
                super::spawn(async move {
                    let _ = pins::show(&engine, &agent, &list, &look).await;
                });
            }
            self.keep_pin_wait(&agent, !mine.is_empty(), &engine, cx);
            for pin in mine.iter().filter(|pin| {
                pin.state == PinState::Answered
                    && pin.after.is_none()
                    && (pin.after_by_hand || !served_here(&pin.url))
            }) {
                self.take_after_by_hand(pin, &engine, cx);
            }
        }
        self.desks
            .update(cx, |desks, _| desks.pins_shown.pictures.trim(&in_sight));
    }

    /// Asks the core for picture `id`, off the window's thread, unless it is kept or on its way.
    fn fetch_pin_picture(&mut self, id: String, cx: &mut Context<Self>) {
        let wanted = self.desks.update(cx, |desks, _| {
            let pictures = &mut desks.pins_shown.pictures;
            !pictures.data.contains_key(&id)
                && !pictures.missing.contains(&id)
                && pictures.fetching.insert(id.clone())
        });
        if !wanted {
            return;
        }
        let fetched = crate::core::Target::here(&self.asks)
            .request_typed::<call::PinPicture>(PinPictureParams { id: id.clone() });
        cx.spawn(async move |bench, cx| {
            let fetched = fetched.await;
            bench
                .update(cx, |bench, cx| {
                    bench.desks.update(cx, |desks, _| {
                        let pictures = &mut desks.pins_shown.pictures;
                        match fetched {
                            Ok(picture) => pictures.keep(
                                id,
                                format!("data:{};base64,{}", picture.mime_type, picture.data),
                            ),
                            Err(_) => {
                                pictures.fetching.remove(&id);
                                pictures.missing.insert(id);
                            }
                        }
                    });
                    bench.show_pins(cx);
                })
                .ok();
        })
        .detach();
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

    /// The after picture of a page this machine serves, from the private view off screen. When
    /// it cannot be had there, the core is told, and the person's own tab takes it later.
    fn take_after_privately(
        &mut self,
        pin: &Pin,
        engine: &Arc<BrowserManager>,
        cx: &mut Context<Self>,
    ) {
        let fresh = self.desks.update(cx, |desks, _| {
            desks.pins_shown.taking.insert(pin.id.clone())
        });
        if !fresh {
            return;
        }
        let Ok(fixed) = sikemux_browser::viewport::Viewport::sized(pin.width, pin.height) else {
            return;
        };
        let (engine, agent, url, selector) = (
            engine.clone(),
            pin.agent_id.clone(),
            pin.url.clone(),
            pin.selector.clone(),
        );
        let target = crate::core::Target::here(&self.asks);
        let id = pin.id.clone();
        super::spawn(async move {
            let taken = engine
                .private_picture_of(&agent, &url, fixed, &selector)
                .await
                .ok();
            target.call_typed::<call::PinAfter>(PinAfterParams {
                id,
                after: taken.map(|jpeg| encoded(&jpeg)),
            });
        });
    }

    /// The after picture from the person's own tab, which shows the pin's page now, when the
    /// element is in sight there.
    fn take_after_by_hand(
        &mut self,
        pin: &Pin,
        engine: &Arc<BrowserManager>,
        cx: &mut Context<Self>,
    ) {
        let hand = format!("hand:{}", pin.id);
        let fresh = self
            .desks
            .update(cx, |desks, _| desks.pins_shown.taking.insert(hand.clone()));
        if !fresh {
            return;
        }
        let (engine, id, agent, selector) = (
            engine.clone(),
            pin.id.clone(),
            pin.agent_id.clone(),
            pin.selector.clone(),
        );
        let target = crate::core::Target::here(&self.asks);
        cx.spawn(async move |bench, cx| {
            let taken = super::run(async move { pins::snapshot(&engine, &agent, &selector).await })
                .await
                .and_then(Result::ok)
                .flatten();
            match taken {
                Some(jpeg) => target.call_typed::<call::PinAfter>(PinAfterParams {
                    id,
                    after: Some(encoded(&jpeg)),
                }),
                None => {
                    bench
                        .update(cx, |bench, cx| {
                            bench
                                .desks
                                .update(cx, |desks, _| desks.pins_shown.taking.remove(&hand))
                        })
                        .ok();
                }
            }
        })
        .detach();
    }

    /// The person clicked an element in snip mode and wrote a note: a picture of the spot is
    /// taken from their own tab, and the pin goes to the desk's agent.
    pub(crate) fn pinned(
        &mut self,
        agent: &str,
        tab_id: String,
        pin: Value,
        cx: &mut Context<Self>,
    ) {
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
        let (agent, area) = (agent.to_owned(), pin["rect"].clone());
        cx.spawn(async move |_, cx| {
            let taken =
                super::run(async move { pins::spot(&engine, &agent, tab_id, &area).await.ok() })
                    .await
                    .flatten();
            params.before = taken.map(|jpeg| encoded(&jpeg));
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

    /// Lets go of the pictures of pin `id`, which leaves the page.
    fn forget_pin_pictures(&mut self, id: &str, cx: &mut Context<Self>) {
        let ids: Vec<String> = (self.this_view())
            .and_then(|view| view.pins.iter().find(|pin| pin.id == id))
            .map(|pin| pin.before.iter().chain(&pin.after).cloned().collect())
            .unwrap_or_default();
        self.desks.update(cx, |desks, _| {
            for picture in &ids {
                desks.pins_shown.pictures.forget(picture);
            }
        });
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
            PinAnswer::LooksRight { id } => {
                self.forget_pin_pictures(&id, cx);
                (
                    Box::pin(target.request_typed::<call::PinLooksRight>(PinParams { id })),
                    "",
                )
            }
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
