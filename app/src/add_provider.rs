//! The dialog that adds a model provider from its base URL and key. The core checks the key,
//! names the provider and says which agent runs it; this host installs that agent meanwhile
//! when it lacks it.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::component::WindowExt;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Animation, AnimationExt, AnyElement, App, AppContext, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Subscription, Window,
};
use serde_json::{json, Value};

use crate::agents::setup::{self, Installing};
use crate::chat::model_picker::{Move, Moving};
use crate::core::Target;
use crate::hosts::Place;
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2H, SPACE_3, TEXT_2XS, TEXT_XS};
use crate::toast;
use crate::ui::{self, dialog};
use crate::workbench::Workbench;

/// How long typing rests before the URL and key are checked.
const SETTLE: Duration = Duration::from_millis(350);

/// The URL and key a check was asked about.
type Asked = (String, String);

pub struct AddProvider {
    target: Target,
    /// Moves the chat the dialog was opened from, while it has not started, onto the provider.
    chat: Option<Move>,
    url: Entity<InputState>,
    key: Entity<InputState>,
    name: Entity<InputState>,
    /// The name the core suggested last, which the field keeps until the person types their own.
    suggested: String,
    /// The core's check of the URL and key it was asked about.
    read: Option<(Asked, Value)>,
    reading: bool,
    adding: bool,
    failure: Option<String>,
    _typing: Vec<Subscription>,
}

fn input(
    placeholder: &'static str,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> Entity<InputState> {
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .masked(masked)
    })
}

impl AddProvider {
    fn new(
        target: Target,
        chat: Option<Move>,
        window: &mut Window,
        cx: &mut Context<Workbench>,
    ) -> Self {
        let url = input("https://api.example.com/v1", false, window, cx);
        let key = input(
            "Not needed for a model server on this host",
            true,
            window,
            cx,
        );
        let name = input("Named from the URL", false, window, cx);
        url.update(cx, |url, cx| url.focus(window, cx));
        let checks =
            |field: &Entity<InputState>, window: &mut Window, cx: &mut Context<Workbench>| {
                cx.subscribe_in(field, window, |bench, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::Change => settle(bench, window, cx),
                        InputEvent::PressEnter { .. } => add(bench, window, cx),
                        _ => {}
                    }
                })
            };
        let typing =
            vec![
                checks(&url, window, cx),
                checks(&key, window, cx),
                cx.subscribe_in(&name, window, |bench, _, event: &InputEvent, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => add(bench, window, cx),
                        _ => cx.notify(),
                    }
                }),
            ];
        Self {
            target,
            chat,
            url,
            key,
            name,
            suggested: String::new(),
            read: None,
            reading: false,
            adding: false,
            failure: None,
            _typing: typing,
        }
    }

    fn asked(&self, cx: &App) -> Asked {
        let value = |field: &Entity<InputState>| field.read(cx).value().trim().to_owned();
        (value(&self.url), value(&self.key))
    }

    /// The check of the URL and key the fields hold now, once the core has given it.
    fn current(&self, cx: &App) -> Option<&Value> {
        let (asked, read) = self.read.as_ref()?;
        (*asked == self.asked(cx)).then_some(read)
    }

    /// The name the provider is added under: the person's, else the core's suggestion.
    fn label(&self, cx: &App) -> String {
        let typed = self.name.read(cx).value().trim().to_owned();
        match typed.is_empty() {
            true => self.suggested.clone(),
            false => typed,
        }
    }
}

impl Workbench {
    /// Opens the dialog, adding to the host in front; `chat` moves the chat it was opened from
    /// onto the provider once it is added.
    pub fn add_provider(
        &mut self,
        chat: Option<Move>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.add_provider_to(self.target(), chat, window, cx);
    }

    pub fn add_provider_to(
        &mut self,
        target: Target,
        chat: Option<Move>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlays.adding_provider = Some(AddProvider::new(target, chat, window, cx));
        cx.notify();
    }
}

/// Checks the URL and key once typing rests on them.
fn settle(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(adding) = bench.overlays.adding_provider.as_mut() else {
        return;
    };
    let asked = adding.asked(cx);
    adding.failure = None;
    adding.reading = !asked.0.is_empty();
    cx.notify();
    if asked.0.is_empty() {
        return;
    }
    let target = adding.target.clone();
    cx.spawn_in(window, async move |bench, cx| {
        cx.background_executor().timer(SETTLE).await;
        let still = bench
            .update(cx, |bench, cx| {
                bench
                    .overlays
                    .adding_provider
                    .as_ref()
                    .is_some_and(|adding| adding.asked(cx) == asked)
            })
            .unwrap_or(false);
        if !still {
            return;
        }
        let (url, key) = asked.clone();
        let read = target
            .request("modelProviderRead", json!({ "baseUrl": url, "key": key }))
            .await
            .unwrap_or_else(|failure| json!({ "failure": readable(failure) }));
        let _ = bench.update_in(cx, |bench, window, cx| {
            let Some(adding) = bench.overlays.adding_provider.as_mut() else {
                return;
            };
            if adding.asked(cx) != asked {
                return;
            }
            adding.reading = false;
            if let Some(label) = read["label"].as_str() {
                let typed = adding.name.read(cx).value().trim().to_owned();
                if typed.is_empty() || typed == adding.suggested {
                    adding
                        .name
                        .update(cx, |name, cx| name.set_value(label, window, cx));
                }
                adding.suggested = label.to_owned();
            }
            let works = read["failure"].is_null() && read["models"].is_array();
            let agent = read["runsWith"].as_str().map(str::to_owned);
            let here = matches!(adding.target.place(), Place::This);
            adding.read = Some((asked, read));
            if let Some(agent) = agent.filter(|agent| works && here && !bench.has_agent(agent)) {
                setup::install_behind(bench, &agent, cx);
            }
            cx.notify();
        });
    })
    .detach();
}

fn close(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.overlays.adding_provider = None;
    cx.notify();
}

fn add(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(adding) = bench.overlays.adding_provider.as_mut() else {
        return;
    };
    let Some(read) = adding.current(cx).cloned() else {
        return;
    };
    let label = adding.label(cx);
    if adding.adding || !read["failure"].is_null() || !read["models"].is_array() || label.is_empty()
    {
        return;
    }
    let (url, key) = adding.asked(cx);
    adding.adding = true;
    let added = adding.target.request(
        "modelProviderAdd",
        json!({ "baseUrl": url, "key": key, "label": label }),
    );
    cx.notify();
    cx.spawn_in(window, async move |bench, cx| {
        let added = added.await;
        let moved = bench.update_in(cx, |bench, window, cx| {
            let moved = match added {
                Ok(provider) => {
                    let label = provider["label"].as_str().unwrap_or("the provider");
                    window.push_notification(
                        toast::success(format!(
                            "Added {label}; its models are in every chat's model menu"
                        )),
                        cx,
                    );
                    let chat = bench
                        .overlays
                        .adding_provider
                        .take()
                        .and_then(|adding| adding.chat);
                    crate::settings::providers_changed(bench, cx);
                    chat.zip(onto(bench, &provider))
                }
                Err(failure) => {
                    if let Some(adding) = bench.overlays.adding_provider.as_mut() {
                        adding.adding = false;
                        adding.failure = Some(readable(failure));
                    }
                    None
                }
            };
            cx.notify();
            moved
        });
        if let Ok(Some((chat, going))) = moved {
            let _ = cx.update(|window, cx| chat(going, window, cx));
        }
    })
    .detach();
}

/// Where a chat goes to run on `provider`: the agent Sikemux runs it with, on that agent's
/// account, starting on the provider's model for it, else its first. A provider serving no
/// model leaves the chat where it is, as the agent could not start on it.
fn onto(bench: &Workbench, provider: &Value) -> Option<Moving> {
    let agent = provider["runsWith"].as_str()?;
    let model = provider["defaults"][agent]
        .as_str()
        .or_else(|| provider["models"][0].as_str())
        .map(str::to_owned)?;
    Some(Moving {
        provider: agent.to_owned(),
        profile: bench
            .accounts
            .current(agent)
            .map(|profile| profile.id.clone()),
        model: Some(model),
        model_provider: provider["id"].as_str().map(str::to_owned),
    })
}

/// A failed request in the person's words: the core words its own refusals, but a host too old
/// to know the call, or a core that went away, says only what it lacked.
fn readable(failure: String) -> String {
    if failure.starts_with("this host does not know") {
        "This host's Sikemux is too old to add providers. Update it, then try again.".into()
    } else if failure == "the core is not connected" {
        "Sikemux lost touch with this host. Try again in a moment.".into()
    } else {
        failure
    }
}

/// Why the agent Sikemux picked runs the provider, from the APIs it speaks.
fn reason(name: &str, read: &Value) -> String {
    let speaks = |api: &str| {
        read["speaks"]
            .as_array()
            .is_some_and(|all| all.iter().any(|each| each == api))
    };
    if speaks("anthropic") {
        format!("{name} speaks Anthropic's API, which Claude Code uses.")
    } else if speaks("responses") {
        format!("{name} speaks OpenAI's Responses API, which Codex uses.")
    } else {
        format!("{name} speaks only chat completions, so Claude Code and Codex can't use it.")
    }
}

/// A thin bar a short stroke runs along while the install has no end in sight; still for a
/// person who asked for less motion.
fn running_bar(t: &Tokens, still: bool) -> AnyElement {
    let (track, ink) = (t.border, t.text_tertiary);
    let bar = move |at: f32| {
        div()
            .relative()
            .h(px(2.))
            .w_full()
            .rounded(px(1.))
            .bg(track)
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .top_0()
                    .h_full()
                    .w(gpui_kit::relative(0.3))
                    .left(gpui_kit::relative(at * 1.3 - 0.3))
                    .bg(ink),
            )
    };
    if still {
        return bar(0.5).into_any_element();
    }
    div()
        .w_full()
        .with_animation(
            "provider-install",
            Animation::new(Duration::from_millis(1400))
                .repeat()
                .with_max_fps(30.),
            move |holder, at| holder.child(bar(at)),
        )
        .into_any_element()
}

/// The dialog, while one is open.
pub fn view(bench: &Workbench, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let adding = bench.overlays.adding_provider.as_ref()?;
    let t = Tokens::get(cx).clone();
    let this = cx.entity();
    let read = adding.current(cx).cloned();
    let name = adding.label(cx);
    let models = read
        .as_ref()
        .and_then(|read| read["models"].as_array())
        .map(Vec::len);
    let failure = adding
        .failure
        .clone()
        .or_else(|| read.as_ref()?["failure"].as_str().map(str::to_owned));
    let ready = models.is_some() && failure.is_none() && !adding.adding && !name.is_empty();
    let keyed = !adding.asked(cx).1.is_empty();
    let check = match (&failure, models, adding.reading) {
        (Some(failure), _, _) => Some((failure.clone(), t.danger)),
        (None, Some(count), _) => Some((
            format!(
                "{} · {count} model{}",
                if keyed { "Key works" } else { "Works" },
                if count == 1 { "" } else { "s" }
            ),
            t.live,
        )),
        (None, None, true) => Some(("Checking…".to_owned(), t.text_tertiary)),
        (None, None, false) => None,
    };
    let runs = read
        .as_ref()
        .filter(|_| ready || adding.adding)
        .and_then(|read| Some((read["runsWith"].as_str()?.to_owned(), read)))
        .map(|(agent, read)| {
            let label = bench.agent_name(&agent).to_owned();
            let state = match (
                bench.has_agent(&agent),
                bench.agent_setup.installing(&agent),
            ) {
                (true, _) => None,
                (false, Some(Installing::Running)) => Some(("Installing", t.text_tertiary)),
                (false, Some(Installing::Failed)) => Some(("Install failed", t.danger)),
                (false, None) => Some(("Not installed", t.text_tertiary)),
            };
            let running = matches!(state, Some(("Installing", _)));
            let failed = matches!(state, Some(("Install failed", _)));
            let open = {
                let (this, agent) = (this.clone(), agent.clone());
                move |_: &gpui_kit::ClickEvent, window: &mut Window, cx: &mut gpui_kit::App| {
                    this.update(cx, |bench, cx| {
                        bench.overlays.adding_provider = None;
                        setup::show_install(bench, &agent, window, cx)
                    })
                }
            };
            div()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .pt(SPACE_2H)
                .border_t_1()
                .border_color(t.border)
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap(px(9.))
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .child(div().pt(px(1.)).child(ui::mark(&agent, 14.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(SPACE_0)
                                .child(
                                    div()
                                        .text_color(t.text_primary)
                                        .child(format!("Runs with {label}")),
                                )
                                .child(
                                    div()
                                        .text_size(crate::theme::ui_text(TEXT_2XS))
                                        .line_height(crate::theme::ui_text(15.))
                                        .text_color(t.text_tertiary)
                                        .child(reason(&name, read)),
                                ),
                        )
                        .children(state.map(|(words, ink)| {
                            div()
                                .id("provider-agent-state")
                                .flex_none()
                                .text_size(crate::theme::ui_text(TEXT_2XS))
                                .text_color(ink)
                                .child(words)
                                .when(failed, |state| state.cursor_pointer().on_click(open))
                        })),
                )
                .when(running, |line| {
                    line.child(div().pl(px(23.)).child(running_bar(&t, cx.reduce_motion())))
                })
        });
    let body = dialog::marked_body()
        .pt(SPACE_1)
        .gap(SPACE_3)
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .line_height(crate::theme::ui_text(17.))
                .text_color(t.text_tertiary)
                .child(
                    "Any provider with an OpenAI-compatible API, or a model server on this host.",
                ),
        )
        .child(dialog::field("Base URL", &adding.url, true, cx))
        .child(dialog::field("API key", &adding.key, false, cx))
        .child(dialog::field("Name", &adding.name, false, cx))
        .children(check.map(|(words, ink)| {
            ui::text(words, TEXT_XS, FontWeight::NORMAL)
                .text_color(ink)
                .whitespace_normal()
        }))
        .children(runs)
        .into_any_element();
    let (adds, closing) = (this.clone(), this);
    let yes: dialog::Answer =
        Rc::new(move |window, cx| adds.update(cx, |bench, cx| add(bench, window, cx)));
    let no: dialog::Answer = Rc::new(move |_, cx| closing.update(cx, close));
    let label: SharedString = match (adding.adding, name.is_empty()) {
        (true, _) => "Adding…".into(),
        (false, false) => format!("Add {name}").into(),
        (false, true) => "Add".into(),
    };
    let foot = dialog::hinted_foot(dialog::escape("cancel", cx), cx)
        .child(dialog::button(
            "provider-cancel",
            "Cancel".into(),
            None,
            no.clone(),
            cx,
        ))
        .child(
            div()
                .when(!ready, |button| button.opacity(0.45))
                .child(dialog::button(
                    "provider-add",
                    label,
                    Some(t.accent),
                    yes.clone(),
                    cx,
                )),
        );
    Some(
        dialog::marked_sheet(Icon::Plus, "Add a provider".into(), body, foot, yes, no, cx)
            .into_any_element(),
    )
}
