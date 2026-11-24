//! A connector's sign-in, in the shared sign-in box over the window: the browser's way in where
//! the connector has one, AWS's profile before it, then the connector's token form.

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, Styled, Subscription, Task, Window,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use sikemux_calls::call;
use sikemux_calls::plugins::{PluginCall, StreamCall};

use crate::core::Target;
use crate::icons::Icon;
use crate::theme::Tokens;
use crate::ui::{self, dialog, sign_in, Clicks};
use crate::workbench::Workbench;

pub(crate) const AWS: &str = "sikemux.aws";

/// A sign-in on its way: the token form's boxes, and the browser's, while it runs.
pub struct Signing {
    plugin: String,
    /// The boxes of every way in, each under its field's name.
    fields: Vec<(String, Entity<InputState>)>,
    _typing: Vec<Subscription>,
    /// The form's second way in is showing.
    other_way: bool,
    /// The names of the form's ticked boxes.
    ticked: Vec<String>,
    /// AWS's profiles, and the one to sign in with.
    profiles: Vec<String>,
    profile: Option<String>,
    busy: bool,
    refused: Option<SharedString>,
    /// The browser sign-in, which dropping stops.
    browser: Option<Task<()>>,
}

#[derive(Deserialize)]
struct Profile {
    name: String,
}

/// Asks `plugin` for `method`, and reads its answer.
pub(crate) fn ask<T: serde::de::DeserializeOwned + 'static>(
    bench: &Workbench,
    plugin: &str,
    method: &str,
    params: Value,
) -> impl std::future::Future<Output = Result<T, SharedString>> + use<T> {
    let asked = Target::here(&bench.asks).request_typed::<call::Plugin>(PluginCall {
        plugin: plugin.to_owned(),
        method: method.to_owned(),
        params,
    });
    async move {
        asked
            .await
            .map_err(SharedString::from)?
            .read()
            .map_err(SharedString::from)
    }
}

/// The connector's mark, as the rest of the app draws it.
pub(crate) fn mark(id: &str) -> Icon {
    id.strip_prefix("sikemux.")
        .and_then(Icon::named)
        .unwrap_or(Icon::Plug)
}

/// Opens the sign-in for the connector `plugin` over the window.
pub fn open(bench: &mut Workbench, plugin: &str, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(plugin) = bench
        .reads
        .plugins
        .all()
        .find(|listed| listed.id == plugin)
        .cloned()
    else {
        return;
    };
    let form = plugin.token_form.as_ref();
    let mut typing = Vec::new();
    let mut fields: Vec<(String, Entity<InputState>)> = Vec::new();
    let other_fields = form.iter().flat_map(|form| &form.other_way);
    for field in
        (form.iter().flat_map(|form| &form.fields)).chain(other_fields.flat_map(|way| &way.fields))
    {
        if typed_in(&fields, &field.name).is_none() {
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(field.placeholder.clone())
                    .masked(field.secret)
            });
            typing.push(cx.subscribe(&input, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
            fields.push((field.name.clone(), input));
        }
    }
    let first = fields.first().filter(|_| !plugin.browser_sign_in);
    if let Some((_, first)) = first {
        first.update(cx, |first, cx| first.focus(window, cx));
    }
    if plugin.id == AWS {
        read_profiles(bench, cx);
    }
    bench.connector_sign_in = Some(Signing {
        plugin: plugin.id,
        fields,
        _typing: typing,
        other_way: false,
        ticked: Vec::new(),
        profiles: Vec::new(),
        profile: None,
        busy: false,
        refused: None,
        browser: None,
    });
    cx.notify();
}

/// Reads AWS's profiles, and picks the one it is signed in as.
fn read_profiles(bench: &Workbench, cx: &mut Context<Workbench>) {
    let profiles = ask::<Vec<Profile>>(bench, AWS, "profiles", Value::Null);
    let status = ask::<Value>(bench, AWS, "status", Value::Null);
    cx.spawn(async move |bench, cx| {
        let profiles: Vec<String> = (profiles.await.unwrap_or_default().into_iter())
            .map(|profile| profile.name)
            .collect();
        let account = status.await.ok().and_then(|status| {
            let profile = status.get("profile")?.as_str()?;
            (!profile.is_empty()).then(|| profile.to_owned())
        });
        let _ = bench.update(cx, |bench, cx| {
            change(bench, cx, |signing| {
                signing.profiles = profiles;
                if signing.profile.is_none() {
                    signing.profile = account;
                }
            });
        });
    })
    .detach();
}

fn change(bench: &mut Workbench, cx: &mut Context<Workbench>, change: impl FnOnce(&mut Signing)) {
    if let Some(signing) = &mut bench.connector_sign_in {
        change(signing);
        cx.notify();
    }
}

fn close(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    bench.connector_sign_in = None;
    cx.notify();
}

fn typed_in<'a>(
    fields: &'a [(String, Entity<InputState>)],
    name: &str,
) -> Option<&'a Entity<InputState>> {
    let (_, input) = fields.iter().find(|(field, _)| field == name)?;
    Some(input)
}

fn switch_way(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change(bench, cx, |signing| {
        signing.other_way = !signing.other_way;
        signing.refused = None;
    });
}

/// Sends what the way in showing holds to the method it names.
fn sign_in_with_token(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(signing) = &bench.connector_sign_in else {
        return;
    };
    let Some(form) = bench.reads.plugins.token_form(&signing.plugin) else {
        return;
    };
    let (method, way) = form.way(signing.other_way);
    let mut typed: Map<String, Value> = way
        .iter()
        .filter_map(|field| {
            let typed = typed_in(&signing.fields, &field.name)?.read(cx).value();
            let value = match field.exact {
                true => typed.to_string(),
                false => typed.trim().to_owned(),
            };
            let value = (!value.is_empty()).then_some(Value::String(value));
            Some((field.name.clone(), value.unwrap_or(Value::Null)))
        })
        .collect();
    for check in &form.checks {
        let on = signing.ticked.contains(&check.name);
        typed.insert(check.name.clone(), Value::Bool(on));
    }
    let signed = ask::<Value>(bench, &signing.plugin, method, typed.into());
    change(bench, cx, |signing| {
        signing.busy = true;
        signing.refused = None;
    });
    cx.spawn(async move |bench, cx| {
        let signed = signed.await.map(|_| ());
        let _ = bench.update(cx, |bench, cx| took_sign_in(bench, signed, cx));
    })
    .detach();
}

/// Starts the connector's sign-in in the browser: its own, or for AWS the profile's single
/// sign-on.
fn sign_in_with_browser(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(signing) = &bench.connector_sign_in else {
        return;
    };
    let plugin = signing.plugin.clone();
    let (method, params) = match plugin.as_str() {
        AWS => ("signIn", json!({ "profile": signing.profile })),
        _ => ("signInWithBrowser", json!({})),
    };
    let call = PluginCall {
        plugin,
        method: method.to_owned(),
        params,
    };
    let mut streaming = Target::here(&bench.asks)
        .stream::<call::PluginStream>(|stream| StreamCall { stream, call });
    let waiting = cx.spawn(async move |bench, cx| {
        let mut failed = None;
        while let Some(part) = streaming.parts.recv().await {
            for item in part.items {
                if let Some(url) = item.get("url").and_then(Value::as_str) {
                    cx.update(|cx| cx.open_url(url));
                }
            }
            if let Some(error) = part.error {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                failed = Some(message.unwrap_or_else(|| error.to_string()));
            }
            if part.done {
                break;
            }
        }
        let signed = failed.map_or(Ok(()), |failed| Err(failed.into()));
        let _ = bench.update(cx, |bench, cx| took_sign_in(bench, signed, cx));
    });
    change(bench, cx, |signing| {
        signing.busy = true;
        signing.refused = None;
        signing.browser = Some(waiting);
    });
}

/// The sign-in finished: the box closes, or says why the service refused it.
fn took_sign_in(
    bench: &mut Workbench,
    signed: Result<(), SharedString>,
    cx: &mut Context<Workbench>,
) {
    match signed {
        Ok(()) => close(bench, cx),
        Err(refused) => change(bench, cx, |signing| {
            signing.busy = false;
            signing.browser = None;
            signing.refused = Some(refused);
        }),
    }
}

fn stop_browser(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    change(bench, cx, |signing| {
        signing.busy = false;
        signing.browser = None;
    });
}

pub(crate) fn on(
    this: &Entity<Workbench>,
    run: impl Fn(&mut Workbench, &mut Context<Workbench>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let this = this.clone();
    move |_, _, cx| this.update(cx, |bench, cx| run(bench, cx))
}

/// The sign-in over the window while one is open.
pub fn dialog(bench: &Workbench, this: &Entity<Workbench>, cx: &mut App) -> Option<AnyElement> {
    let signing = bench.connector_sign_in.as_ref()?;
    let plugin = bench
        .reads
        .plugins
        .all()
        .find(|plugin| plugin.id == signing.plugin)?;
    let t = Tokens::get(cx).clone();
    let name = plugin.name.clone();
    let form = plugin.token_form.as_ref();
    let browser = plugin.browser_sign_in || plugin.id == AWS;
    let browsing = signing.browser.is_some();
    let mut screen = sign_in::Screen::new(
        "connector-sign-in",
        ui::picture(mark(&plugin.id), 26., t.text_primary),
        t!("Sign in to {name}", name = name),
    )
    .lede(form.map_or_else(
        || t!("Tools for your agents to use {name} for you.", name = name),
        |form| form.lede.clone(),
    ));
    if plugin.id == AWS {
        let chosen = signing.profile.clone().unwrap_or_default();
        let this = this.clone();
        screen = screen.child(sign_in::pick(
            "connector-aws-profile",
            match chosen.is_empty() {
                true => t!("Pick a profile").to_owned(),
                false => chosen.clone(),
            },
            (signing.profiles.iter())
                .map(|name| ui::Pick::new(name.clone(), name.clone()))
                .collect(),
            Some(chosen.into()),
            signing.busy,
            move |key, _, cx| {
                let profile = key.to_string();
                this.update(cx, |bench, cx| {
                    change(bench, cx, |signing| signing.profile = Some(profile))
                })
            },
            cx,
        ));
    }
    if browser {
        screen = screen.child(match browsing {
            true => sign_in::waiting(
                "connector-browser",
                t!("Finish signing in in your browser"),
                on(this, stop_browser),
                cx,
            )
            .into_any_element(),
            false => ui::Button::new(
                "connector-browser",
                t!("Continue with {name}", name = name),
                ui::Variant::Primary,
            )
            .wide()
            .icon(mark(&plugin.id))
            .disabled(signing.busy || (plugin.id == AWS && signing.profile.is_none()))
            .on_click(on(this, sign_in_with_browser))
            .into_any_element(),
        });
    }
    if let Some(form) = form {
        if browser {
            screen = screen.child(sign_in::or(t!("or paste a token"), cx));
        }
        let (_, way) = form.way(signing.other_way);
        let boxes: Vec<_> = way
            .iter()
            .filter_map(|field| Some((field, typed_in(&signing.fields, &field.name)?)))
            .collect();
        for (field, input) in &boxes {
            let typed = sign_in::Field::new(field.label.clone(), input);
            screen = screen.child(if field.secret { typed.mono() } else { typed });
        }
        for check in &form.checks {
            let name = check.name.clone();
            let this = this.clone();
            screen = screen.child(
                div().w_full().flex().child(
                    Checkbox::new(SharedString::from(format!(
                        "connector-check-{}",
                        check.name
                    )))
                    .label(check.label.clone())
                    .checked(signing.ticked.contains(&check.name))
                    .on_click(move |on: &bool, _, cx| {
                        let (name, on) = (name.clone(), *on);
                        this.update(cx, |bench, cx| {
                            change(bench, cx, |signing| {
                                signing.ticked.retain(|ticked| *ticked != name);
                                if on {
                                    signing.ticked.push(name);
                                }
                            })
                        })
                    }),
                ),
            );
        }
        let filled = boxes
            .iter()
            .all(|(_, input)| !input.read(cx).value().trim().is_empty());
        screen = screen.child(
            ui::Button::new(
                "connector-token",
                if signing.busy && !browsing {
                    t!("Signing in…")
                } else {
                    t!("Sign in")
                },
                match browser {
                    true => ui::Variant::Outline,
                    false => ui::Variant::Primary,
                },
            )
            .wide()
            .disabled(signing.busy || !filled)
            .on_click(on(this, sign_in_with_token)),
        );
    }
    match (&signing.refused, &plugin.problem) {
        (Some(refused), _) => screen = screen.child(sign_in::error(refused.clone(), cx)),
        (None, Some(problem)) => screen = screen.child(sign_in::note(problem.clone(), cx)),
        (None, None) => {}
    }
    let server = typed_in(&signing.fields, "host")
        .map(|input| input.read(cx).value().to_string())
        .unwrap_or_default();
    let page = form.and_then(|form| {
        form.token_page(&server)
            .map(|page| (page, form.token_page_label().to_owned()))
    });
    let switch = form
        .and_then(|form| form.other_way.as_ref())
        .map(|way| match signing.other_way {
            true => way.back.clone(),
            false => way.offer.clone(),
        });
    screen = screen.child(
        sign_in::links()
            .when_some(switch, |links, words| {
                links.child(
                    sign_in::link("connector-other-way", words, cx).clicks(on(this, switch_way)),
                )
            })
            .when_some(page, |links, (page, label)| {
                links.child(
                    sign_in::link("connector-token-page", label, cx)
                        .clicks(move |_, _, cx| cx.open_url(&page)),
                )
            })
            .child(sign_in::link("connector-cancel", t!("Cancel"), cx).clicks(on(this, close))),
    );
    let close: dialog::Answer = {
        let this = this.clone();
        std::rc::Rc::new(move |_, cx| this.update(cx, close))
    };
    let go: dialog::Answer = {
        let this = this.clone();
        std::rc::Rc::new(move |_, cx| this.update(cx, sign_in_with_token))
    };
    let body = div().px(px(4.)).pt(px(4.)).child(screen);
    Some(dialog::floating(px(440.), body, go, close).into_any_element())
}

/// Opens the sign-in for `plugin` over the window already waiting on the browser, where the
/// connector signs in there.
pub fn open_in_browser(
    bench: &mut Workbench,
    plugin: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    open(bench, plugin, window, cx);
    let browser =
        (bench.reads.plugins.all()).any(|listed| listed.id == plugin && listed.browser_sign_in);
    if browser {
        sign_in_with_browser(bench, cx);
    }
}

/// Where the sign-in open for a connector stands.
pub enum Stand {
    Open,
    Busy,
    Refused(SharedString),
}

/// Where the sign-in for `plugin` stands, while one is open over the window.
pub fn stand(bench: &Workbench, plugin: &str) -> Option<Stand> {
    let signing = bench
        .connector_sign_in
        .as_ref()
        .filter(|signing| signing.plugin == plugin)?;
    Some(match (&signing.refused, signing.busy) {
        (Some(refused), _) => Stand::Refused(refused.clone()),
        (None, true) => Stand::Busy,
        (None, false) => Stand::Open,
    })
}
