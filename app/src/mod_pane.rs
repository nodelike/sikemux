//! A mod's pane: the page of a panel the person or an agent made, read from the core, filling
//! the pane on the app's ground, and read again whenever the mod's files move. Its page reaches
//! the core through `window.sikemux`, the bridge in `bridge.js`: what it reads and writes is the
//! core's to allow, and a watch reads again only when what it reads moves, while the pane shows.

use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

use gpui_kit::{
    div, AnyElement, AppContext, Context, IntoElement, ParentElement, Render, SharedString, Styled,
    WeakEntity, Window,
};
use serde_json::{json, Value};
use sikemux_core::server::mods::follows;
pub use sikemux_core::server::mods::Moved;
use sikemux_view::layout::PaneKind;

use crate::core::Target;
use crate::hosts::Place;
use crate::page_view::{PageEvent, Room};
use crate::theme::{Tokens, TEXT_XS};
use crate::workbench::{Strip, Workbench};
use crate::{page_view, theme};
use sikemux_core::server::call;
use sikemux_core::server::mods::{BadgeParams, ModParams};

const BRIDGE: &str = include_str!("mod_pane/bridge.js");
/// How long a burst of changes gathers before the watches it moved read again.
const GATHER: Duration = Duration::from_millis(300);
const MAX_WATCHES: usize = 32;
const MAX_PARAMS_BYTES: usize = 16 * 1024;

pub struct Watched {
    name: String,
    params: Value,
    /// The last answer the page was given, hashed, so an unchanged one is not sent again.
    last: Option<u64>,
    dirty: bool,
    reading: bool,
}

pub struct ModPane {
    /// The mod's id, its folder's name.
    pub id: String,
    pub core: Target,
    /// The folder of the project whose strip holds the pane.
    project: Option<String>,
    bench: WeakEntity<Workbench>,
    page: Option<page_view::View>,
    problem: Option<SharedString>,
    reading: bool,
    /// Whether the pane's tab is in front.
    shown: bool,
    /// The mod's files moved while the pane was not shown.
    stale: bool,
    watches: BTreeMap<u64, Watched>,
    gathering: bool,
    /// Counts the pages shown, so an answer meant for one shown before is dropped.
    generation: u64,
}

impl ModPane {
    pub fn new(
        id: String,
        core: Target,
        project: Option<String>,
        bench: WeakEntity<Workbench>,
    ) -> Self {
        Self {
            id,
            core,
            project,
            bench,
            page: None,
            problem: None,
            reading: false,
            shown: true,
            stale: true,
            watches: BTreeMap::new(),
            gathering: false,
            generation: 0,
        }
    }

    pub fn show(&mut self, cx: &mut Context<Self>) {
        if std::mem::replace(&mut self.shown, true) {
            return;
        }
        self.run("__sikemux.shown(true)");
        self.gather(cx);
        cx.notify();
    }

    pub fn hide(&mut self) {
        if std::mem::replace(&mut self.shown, false) {
            self.run("__sikemux.shown(false)");
        }
    }

    /// The mod's files moved: reads its page again now, or once its tab comes to the front.
    pub fn files_moved(&mut self, cx: &mut Context<Self>) {
        self.stale = true;
        if self.shown {
            cx.notify();
        }
    }

    /// Something a watch may read moved: the watches that read it read again once the pane shows.
    pub fn moved(&mut self, moved: Moved, cx: &mut Context<Self>) {
        let mut any = false;
        for watch in self.watches.values_mut() {
            if follows(&watch.name) == Some(moved) {
                watch.dirty = true;
                any = true;
            }
        }
        if any {
            self.gather(cx);
        }
    }

    pub fn retheme(&self, cx: &Context<Self>) {
        let rule = serde_json::to_string(&theme_rule(cx)).unwrap_or_default();
        self.run(&format!(
            "(()=>{{const style=document.getElementById(\"sikemux-theme\");if(style)style.textContent={rule};}})()"
        ));
    }

    fn run(&self, script: &str) {
        if let Some(page) = &self.page {
            page.run(script);
        }
    }

    fn read_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.reading || !self.stale {
            return;
        }
        self.reading = true;
        self.stale = false;
        let answer = self.core.request_typed::<call::ModPage>(ModParams {
            id: self.id.clone(),
        });
        cx.spawn_in(window, async move |this, cx| {
            let answer = answer.await;
            let _ = this.update_in(cx, |pane, window, cx| {
                pane.reading = false;
                match answer {
                    Ok(answer) => {
                        let html = answer.html.as_str();
                        pane.problem = None;
                        pane.present(html, window, cx);
                    }
                    Err(problem) => pane.problem = Some(problem.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn present(&mut self, html: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.watches.clear();
        self.generation += 1;
        let html = sikemux_core::pages::themed(html, &theme_rule(cx), Some(BRIDGE));
        if let Some(page) = &self.page {
            page.show(&html);
            return;
        }
        let (events, mut heard) = tokio::sync::mpsc::unbounded_channel();
        self.page = page_view::open(&html, Room::Panel, events, window, cx);
        cx.spawn(async move |this, cx| {
            while let Some(event) = heard.recv().await {
                let heard = this.update(cx, |pane, cx| match event {
                    PageEvent::Link(url) => cx.open_url(&url),
                    PageEvent::Mod(message) => pane.asked(message, cx),
                    PageEvent::Height(_) | PageEvent::Wheel(_) => {}
                });
                if heard.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn asked(&mut self, message: Value, cx: &mut Context<Self>) {
        let id = message["id"].as_u64().unwrap_or_default();
        let name = message["name"].as_str().unwrap_or_default().to_owned();
        let params = message["params"].clone();
        if params.to_string().len() > MAX_PARAMS_BYTES {
            return self.answer_to(self.generation, id, Err("those params are too long".into()));
        }
        match message["op"].as_str() {
            Some("read") => {
                let (read, generation) = (
                    self.core.request("modRead", self.asking(&name, params)),
                    self.generation,
                );
                cx.spawn(async move |this, cx| {
                    let answer = read.await;
                    let _ = this.update(cx, |pane, _| pane.answer_to(generation, id, answer));
                })
                .detach();
            }
            Some("write") => self.write(id, name, params, cx),
            Some("watch") if follows(&name).is_some() => {
                if self.watches.len() >= MAX_WATCHES {
                    return self.watched(id, Err("a mod watches at most 32 things".into()));
                }
                self.watches.insert(
                    id,
                    Watched {
                        name,
                        params,
                        last: None,
                        dirty: true,
                        reading: false,
                    },
                );
                self.gather(cx);
            }
            Some("watch") => {
                let (name, params) = (json!(name), params.to_string());
                self.run(&format!("__sikemux.timed({id},{name},{params})"));
            }
            Some("unwatch") => {
                self.watches.remove(&id);
            }
            Some("badge") => {
                let badge = message["text"].as_str();
                self.core.call_typed::<call::SetModBadge>(BadgeParams {
                    id: self.id.clone(),
                    badge: badge.map(str::to_owned),
                });
            }
            _ => {}
        }
    }

    fn asking(&self, name: &str, params: Value) -> Value {
        json!({ "id": self.id, "name": name, "params": params, "project": self.project })
    }

    /// Runs a write, asking the person first when they have not allowed the mod's writes.
    fn write(&mut self, id: u64, name: String, params: Value, cx: &mut Context<Self>) {
        let asked = self.asking(&name, params);
        let (write, generation) = (
            self.core.request("modWrite", asked.clone()),
            self.generation,
        );
        cx.spawn(async move |this, cx| {
            let answer = write.await;
            let _ = this.update(cx, |pane, cx| match answer {
                Ok(answer) if answer.get("ask").is_some() => {
                    pane.ask(generation, id, &answer["ask"], asked, cx)
                }
                Ok(answer) => pane.answer_to(generation, id, Ok(answer["done"].clone())),
                Err(error) => pane.answer_to(generation, id, Err(error)),
            });
        })
        .detach();
    }

    fn ask(&self, generation: u64, id: u64, ask: &Value, asked: Value, cx: &mut Context<Self>) {
        let name = ask["name"].as_str().unwrap_or(&self.id).to_owned();
        let writes: Vec<String> = serde_json::from_value(ask["writes"].clone()).unwrap_or_default();
        let listed: Vec<String> = writes.iter().map(|write| said(write)).collect();
        let body = format!(
            "{name} asks to {}. It may do so whenever it is open, until its mod.json asks for more. Settings, Mods takes this back.",
            listed.join(", ")
        );
        let pane = cx.weak_entity();
        let _ = self.bench.update(cx, |bench, cx| {
            bench.confirm(
                format!("Allow {name} to make changes?"),
                body,
                "Allow",
                false,
                move |_, allowed, _, cx| {
                    let (writes, asked) = (writes.clone(), asked.clone());
                    let _ = pane.update(cx, |pane, cx| {
                        pane.allowed((generation, id), allowed, writes, asked, cx)
                    });
                },
                cx,
            )
        });
    }

    fn allowed(
        &mut self,
        (generation, id): (u64, u64),
        allowed: bool,
        writes: Vec<String>,
        asked: Value,
        cx: &mut Context<Self>,
    ) {
        if !allowed {
            let refused = Err("the person did not allow this mod's writes".into());
            return self.answer_to(generation, id, refused);
        }
        let allow = self
            .core
            .request("allowMod", json!({ "id": self.id, "writes": writes }));
        let core = self.core.clone();
        cx.spawn(async move |this, cx| {
            let answer = match allow.await {
                Ok(_) => core.request("modWrite", asked).await,
                Err(error) => Err(error),
            };
            let _ = this.update(cx, |pane, _| {
                let answer = match answer {
                    Ok(answer) if answer.get("done").is_some() => Ok(answer["done"].clone()),
                    Ok(_) => Err("this mod's writes are not allowed".into()),
                    Err(error) => Err(error),
                };
                pane.answer_to(generation, id, answer)
            });
        })
        .detach();
    }

    /// Answers the request `id` of the page shown as `generation`, unless another replaced it.
    fn answer_to(&self, generation: u64, id: u64, answer: Result<Value, String>) {
        if generation == self.generation {
            self.run(&format!("__sikemux.answer({id},{})", outcome(answer)));
        }
    }

    fn watched(&self, id: u64, answer: Result<Value, String>) {
        self.run(&format!("__sikemux.watched({id},{})", outcome(answer)));
    }

    /// Reads the moved watches again once a burst of changes has passed, while the pane shows.
    fn gather(&mut self, cx: &mut Context<Self>) {
        if self.gathering || !self.shown {
            return;
        }
        self.gathering = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(GATHER).await;
            let _ = this.update(cx, |pane, cx| {
                pane.gathering = false;
                pane.read_watches(cx);
            });
        })
        .detach();
    }

    fn read_watches(&mut self, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let due: Vec<(u64, Value)> = self
            .watches
            .iter_mut()
            .filter(|(_, watch)| watch.dirty && !watch.reading)
            .map(|(id, watch)| {
                watch.dirty = false;
                watch.reading = true;
                (*id, json!({ "name": watch.name, "params": watch.params }))
            })
            .collect();
        for (id, asked) in due {
            let name = asked["name"].as_str().unwrap_or_default();
            let read = self
                .core
                .request("modRead", self.asking(name, asked["params"].clone()));
            let generation = self.generation;
            cx.spawn(async move |this, cx| {
                let answer = read.await;
                let _ = this.update(cx, |pane, cx| {
                    if pane.generation == generation {
                        pane.read_watch(id, answer, cx);
                    }
                });
            })
            .detach();
        }
    }

    fn read_watch(&mut self, id: u64, answer: Result<Value, String>, cx: &mut Context<Self>) {
        let Some(watch) = self.watches.get_mut(&id) else {
            return;
        };
        watch.reading = false;
        let mut hasher = DefaultHasher::new();
        answer.as_ref().map(Value::to_string).hash(&mut hasher);
        let hash = hasher.finish();
        let fresh = watch.last.replace(hash) != Some(hash);
        if watch.dirty {
            self.gather(cx);
        }
        if fresh {
            self.watched(id, answer);
        }
    }
}

/// An answer as the bridge's script takes it: the error, then the value.
fn outcome(answer: Result<Value, String>) -> String {
    match answer {
        Ok(value) => format!("null,{value}"),
        Err(error) => format!("{},null", json!(error)),
    }
}

/// A write as the person is asked about it.
fn said(write: &str) -> String {
    match write {
        "agents.send" => "message agents".into(),
        "board.add" => "add cards to the agent board".into(),
        "board.update" => "change cards on the agent board".into(),
        other => other.to_owned(),
    }
}

fn theme_rule(cx: &Context<ModPane>) -> String {
    let t = Tokens::get(cx);
    theme::page_rule(t, cfg!(target_os = "linux").then(|| theme::page_ground(t)))
}

impl Render for ModPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.shown {
            self.read_page(window, cx);
        }
        let drawn: AnyElement = match (&self.page, &self.problem) {
            (_, Some(problem)) => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px_6()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(Tokens::get(cx).text_tertiary)
                .child(problem.clone())
                .into_any_element(),
            (Some(page), None) => gpui_native_view::native_view(
                SharedString::from(format!("mod-{}", self.id)),
                &page.surface,
            )
            .size_full()
            .into_any_element(),
            (None, None) => div().size_full().into_any_element(),
        };
        div().size_full().child(drawn)
    }
}

impl Workbench {
    /// Keeps a pane for each mod view of the strip in front, showing those of the tab in front
    /// and pausing the rest, and lets go of the others.
    pub(crate) fn mod_panes(&mut self, cx: &mut Context<Self>) {
        let views_in = |pane: &sikemux_view::layout::Pane| -> Vec<(String, String)> {
            crate::panes::views(pane)
                .filter(|view| view.kind == PaneKind::Mod)
                .map(|view| (view.id.clone(), view.target.clone()))
                .collect()
        };
        let shown: Vec<(String, String)> = self
            .strip()
            .map(|strip| {
                self.tabs(&strip)
                    .iter()
                    .flat_map(|tab| views_in(&tab.pane))
                    .collect()
            })
            .unwrap_or_default();
        let in_front = self
            .tab()
            .map(|tab| views_in(&tab.pane))
            .unwrap_or_default();
        let core = self.target();
        let project = match self.strip() {
            Some(Strip::Project(_)) => self
                .project()
                .map(|project| project.path.to_string_lossy().into_owned()),
            _ => None,
        };
        self.mod_panes.retain(|view, pane| {
            let kept = shown.iter().any(|(id, target)| {
                let pane = pane.read(cx);
                id == view && &pane.id == target && pane.core.place() == core.place()
            });
            match kept && in_front.iter().any(|(id, _)| id == view) {
                true => pane.update(cx, |pane, cx| pane.show(cx)),
                false => pane.update(cx, |pane, _| pane.hide()),
            }
            kept
        });
        for (view, id) in shown {
            if let std::collections::hash_map::Entry::Vacant(slot) = self.mod_panes.entry(view) {
                let bench = cx.weak_entity();
                let pane = crate::mod_pane::ModPane::new(id, core.clone(), project.clone(), bench);
                slot.insert(cx.new(|_| pane));
            }
        }
    }

    /// The core at `place` said the files of the mods in `paths` moved, or of any mod without.
    pub(crate) fn mods_moved(
        &mut self,
        place: &Place,
        paths: Option<&[String]>,
        cx: &mut Context<Self>,
    ) {
        for pane in self.mod_panes.values() {
            let moved = {
                let pane = pane.read(cx);
                pane.core.place() == place && paths.is_none_or(|paths| paths.contains(&pane.id))
            };
            if moved {
                pane.update(cx, |pane, cx| pane.files_moved(cx));
            }
        }
    }

    /// Something the mods on `place` may watch moved.
    pub(crate) fn mods_follow(
        &mut self,
        place: &Place,
        moved: crate::mod_pane::Moved,
        cx: &mut Context<Self>,
    ) {
        for pane in self.mod_panes.values() {
            if pane.read(cx).core.place() == place {
                pane.update(cx, |pane, cx| pane.moved(moved, cx));
            }
        }
    }
}
