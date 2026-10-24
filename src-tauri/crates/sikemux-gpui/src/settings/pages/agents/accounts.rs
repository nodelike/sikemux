//! Each agent's accounts: who each is signed in as, which one is in use, and
//! the dialog that adds one through the agent's own login.

use std::rc::Rc;

use gpui_kit::base::Popover;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, Context, Entity, IntoElement, ParentElement, SharedString,
    Styled, Window,
};
use sikemux_core::agent_accounts::Profile;

use crate::accounts::{self, SignIn, Status};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{self, Tokens};
use crate::ui::{self, dialog, Menu, MenuItem};
use crate::workbench::Workbench;

/// What the account is called: a built-in one still under its agent's name goes by
/// the name on the account instead.
fn name(bench: &Workbench, profile: &Profile, status: Option<&Status>) -> String {
    let unnamed = profile.id.starts_with("builtin-")
        && profile.name == super::label(bench, &profile.provider);
    status
        .and_then(|status| status.name.clone())
        .filter(|_| unnamed)
        .unwrap_or_else(|| profile.name.clone())
}

fn plan(plan: &str) -> String {
    plan.split(['_', '-'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut letters = part.chars();
            letters
                .next()
                .map(|first| first.to_uppercase().chain(letters).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Something a sign-in's button does.
type Run = Rc<dyn Fn(&mut Workbench, &mut Window, &mut Context<Workbench>)>;

/// The buttons of a sign-in while it runs or after it failed, or None when there is none.
fn signing(bench: &Workbench, id: &str, this: &Entity<Workbench>, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let button = |key: &str, label: &'static str, run: Run| {
        let this = this.clone();
        ui::Button::new(
            SharedString::from(format!("{key}-{id}")),
            label,
            ui::Variant::Outline,
        )
        .on_click(move |_, window, cx| {
            let run = run.clone();
            this.update(cx, |bench, cx| run(bench, window, cx))
        })
    };
    let owned = id.to_owned();
    Some(match bench.accounts.signing.get(id)? {
        SignIn::Waiting { url, code } => div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .text_size(crate::theme::ui_text(11.5))
                    .text_color(t.text_tertiary)
                    .child("Finish signing in in your browser"),
            )
            .children(url.clone().map(|url| {
                button(
                    "sign-in-page",
                    "Open page",
                    Rc::new(move |_, _, cx| cx.open_url(&url)),
                )
            }))
            .when(url.is_some(), |row| match code {
                Some(field) => row
                    .child(div().w(px(180.)).child(ui::Field::new(field)))
                    .child({
                        let id = owned.clone();
                        button(
                            "sign-in-send-code",
                            "Send",
                            Rc::new(move |bench, _, cx| accounts::submit_code(bench, &id, cx)),
                        )
                    }),
                None => row.child({
                    let id = owned.clone();
                    button(
                        "sign-in-code",
                        "Paste code",
                        Rc::new(move |bench, window, cx| {
                            accounts::ask_code(bench, &id, window, cx)
                        }),
                    )
                }),
            })
            .child({
                let id = owned.clone();
                button(
                    "sign-in-cancel",
                    "Cancel",
                    Rc::new(move |bench, _, _| accounts::cancel_sign_in(bench, &id)),
                )
            })
            .into_any_element(),
        SignIn::Failed(message) => div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .max_w(px(240.))
                    .truncate()
                    .text_size(crate::theme::ui_text(11.5))
                    .text_color(t.danger)
                    .child(message.clone()),
            )
            .child({
                let id = owned.clone();
                button(
                    "sign-in-retry",
                    "Try again",
                    Rc::new(move |bench, _, cx| {
                        accounts::dismiss_failure(bench, &id);
                        accounts::sign_in(bench, &id, cx);
                    }),
                )
            })
            .into_any_element(),
    })
}

fn account_row(
    bench: &Workbench,
    profile: &Profile,
    in_use: bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let status = accounts::status(bench, profile);
    let known = status.and_then(|status| status.as_ref().ok());
    let name = name(bench, profile, known);
    let signed_in = known.is_some_and(|status| status.signed_in);
    let who: SharedString = match status {
        None => "Checking…".into(),
        Some(Err(_)) => "Could not ask who is signed in".into(),
        Some(Ok(status)) if status.signed_in => {
            let parts: Vec<String> = [
                status.email.clone().or_else(|| status.organization.clone()),
                status.plan.as_deref().map(plan),
            ]
            .into_iter()
            .flatten()
            .collect();
            match parts.is_empty() {
                true => "Signed in".into(),
                false => parts.join(" · ").into(),
            }
        }
        Some(Ok(_)) => "Signed out".into(),
    };
    let id = profile.id.clone();
    let control: AnyElement = match signing(bench, &id, this, cx) {
        Some(signing) => signing,
        None if known.is_some() && !signed_in => {
            let (this, id) = (this.clone(), id.clone());
            ui::Button::new(
                SharedString::from(format!("account-sign-in-{id}")),
                "Sign in",
                ui::Variant::Outline,
            )
            .on_click(move |_, _, cx| {
                let id = id.clone();
                this.update(cx, |bench, cx| accounts::sign_in(bench, &id, cx))
            })
            .into_any_element()
        }
        None if !in_use => {
            let (this, profile) = (this.clone(), profile.clone());
            ui::Button::new(
                SharedString::from(format!("account-use-{id}")),
                "Use",
                ui::Variant::Outline,
            )
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| accounts::select(bench, &profile, cx))
            })
            .into_any_element()
        }
        None => div().into_any_element(),
    };
    let removable = !profile.id.starts_with("builtin-");
    let more = (signed_in || removable).then(|| {
        let (sign_out, remove) = (this.clone(), this.clone());
        let (out_id, remove_id) = (id.clone(), id.clone());
        let remove_label: SharedString = format!("Remove {name}").into();
        Popover::new(SharedString::from(format!("account-more-{id}")))
            .anchor(Anchor::TopRight)
            .offset(px(4.))
            .trigger(ui::IconButton::new(
                SharedString::from(format!("account-more-button-{id}")),
                Icon::More,
                format!("More for {name}"),
            ))
            .content(move |_, _, cx| {
                let popover = cx.entity();
                let (sign_out, remove) = (sign_out.clone(), remove.clone());
                let (out_id, remove_id) = (out_id.clone(), remove_id.clone());
                let shut = popover.clone();
                Menu::new()
                    .when(signed_in, |menu| {
                        menu.child(
                            MenuItem::new("account-sign-out", "Sign out")
                                .icon(Icon::LogOut)
                                .on_click(move |_, window, cx| {
                                    let id = out_id.clone();
                                    sign_out
                                        .update(cx, |bench, cx| accounts::sign_out(bench, &id, cx));
                                    popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                                }),
                        )
                    })
                    .when(removable, |menu| {
                        menu.child(
                            MenuItem::new("account-remove", remove_label.clone())
                                .icon(Icon::Trash)
                                .on_click(move |_, window, cx| {
                                    let id = remove_id.clone();
                                    remove.update(cx, |bench, cx| accounts::remove(bench, &id, cx));
                                    shut.update(cx, |popover, cx| popover.dismiss(window, cx));
                                }),
                        )
                    })
            })
    });
    let label = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(name.clone())
        .when(in_use, |label| {
            label.child(
                div()
                    .px(px(5.))
                    .rounded(theme::RADIUS_1)
                    .bg(t.surface_chosen)
                    .text_size(crate::theme::ui_text(10.5))
                    .text_color(t.text_secondary)
                    .child("In use"),
            )
        });
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(14.))
        .py(px(10.))
        .border_b_1()
        .border_color(t.line_soft)
        .child(accounts::avatar(profile, &name, 24.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(crate::theme::ui_text(13.))
                        .text_color(t.text_primary)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(11.5))
                        .text_color(if known.is_some() && !signed_in {
                            t.warn
                        } else {
                            t.text_tertiary
                        })
                        .child(who),
                ),
        )
        .child(control)
        .children(more)
        .into_any_element()
}

pub fn render(
    bench: &Workbench,
    provider: &'static str,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let title = if provider == "claude" {
        "Claude accounts"
    } else {
        "Codex accounts"
    };
    let list = bench.accounts.of(provider);
    let current = bench
        .accounts
        .current(provider)
        .map(|profile| profile.id.clone());
    let count = list.len();
    let auto = (count > 1).then(|| {
        let this = this.clone();
        let maker = if provider == "claude" {
            "Anthropic's"
        } else {
            "OpenAI's"
        };
        rows::row_with(
            None,
            "Auto-rotate at limits",
            Some(format!("Check {maker} terms first.").into()),
            Switch::new(SharedString::from(format!("auto-rotate-{provider}")))
                .checked(bench.accounts.auto_switch(provider))
                .on_change(move |on: &bool, _, cx| {
                    let on = *on;
                    this.update(cx, |bench, cx| {
                        accounts::set_auto_switch(bench, provider, on, cx)
                    })
                }),
            cx,
        )
    });
    let add = {
        let this = this.clone();
        div().p(px(10.)).child(
            ui::Button::new(
                SharedString::from(format!("account-add-{provider}")),
                "Add account",
                ui::Variant::Dashed,
            )
            .icon(Icon::Plus)
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| {
                    accounts::open_dialog(bench, provider, window, cx)
                })
            }),
        )
    };
    let body = div()
        .children(list.iter().map(|profile| {
            account_row(
                bench,
                profile,
                current.as_deref() == Some(profile.id.as_str()),
                this,
                cx,
            )
        }))
        .children(auto)
        .child(add)
        .when(bench.accounts.book.is_none(), |body| {
            body.child(rows::note("Reading accounts…", cx))
        });
    rows::section(
        title,
        Some(match count {
            1 => "1 account".into(),
            count => format!("{count} accounts").into(),
        }),
        None,
        lit(title),
        body,
        cx,
    )
}

/// The add-account dialog while it is open.
pub fn dialog(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let open = bench.accounts.dialog.as_ref()?;
    let t = Tokens::get(cx).clone();
    let label = super::label(bench, &open.provider);
    let account = open
        .account
        .as_deref()
        .and_then(|id| bench.accounts.profile(id));
    let signing = open
        .account
        .as_deref()
        .and_then(|id| bench.accounts.signing.get(id));
    let close: dialog::Answer = {
        let this = this.clone();
        Rc::new(move |_, cx| this.update(cx, accounts::close_dialog))
    };
    let (title, body): (String, AnyElement) = match (account, signing) {
        (None, _) => {
            let signed_out: Vec<&Profile> = bench
                .accounts
                .of(&open.provider)
                .into_iter()
                .filter(|profile| {
                    accounts::status(bench, profile)
                        .and_then(|status| status.as_ref().ok())
                        .is_some_and(|status| !status.signed_in)
                })
                .collect();
            let again = (!signed_out.is_empty()).then(|| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(11.5))
                            .text_color(t.text_tertiary)
                            .child("or sign in again"),
                    )
                    .children(signed_out.into_iter().map(|profile| {
                        let (this, id) = (this.clone(), profile.id.clone());
                        ui::Button::new(
                            SharedString::from(format!("sign-in-again-{id}")),
                            format!("{} · Signed out", profile.name),
                            ui::Variant::Outline,
                        )
                        .on_click(move |_, _, cx| {
                            let id = id.clone();
                            this.update(cx, |bench, cx| accounts::sign_in_again(bench, &id, cx))
                        })
                    }))
            });
            (
                format!("Add a {label} account"),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(format!("Sign in with {label}'s own login in your browser."))
                    .child(ui::Field::new(&open.name))
                    .children(again)
                    .into_any_element(),
            )
        }
        (Some(profile), Some(_)) => (
            format!("Sign in to {}", profile.name),
            signing_body(bench, &profile.id, this, cx),
        ),
        (Some(profile), None) => {
            let status = accounts::status(bench, profile).and_then(|status| status.as_ref().ok());
            let who = status
                .and_then(|status| status.email.clone().or_else(|| status.name.clone()))
                .unwrap_or_else(|| "Signed in".into());
            (
                format!("{} is signed in", profile.name),
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(accounts::avatar(profile, &profile.name, 28.))
                    .child(who)
                    .into_any_element(),
            )
        }
    };
    let naming = account.is_none();
    let done = account.is_some() && signing.is_none();
    let busy = open.busy;
    let empty = open.name.read(cx).value().trim().is_empty();
    let brand = super::kind(bench, &open.provider)
        .and_then(|kind| kind["brand"].as_str())
        .unwrap_or("#8b8898")
        .to_owned();
    let go: dialog::Answer = {
        let this = this.clone();
        Rc::new(move |_, cx| {
            let brand = brand.clone();
            this.update(cx, |bench, cx| {
                if naming {
                    accounts::add(bench, brand, cx);
                } else if done {
                    accounts::close_dialog(bench, cx);
                }
            })
        })
    };
    let foot = div()
        .when(!done, |foot| {
            foot.child(dialog::button(
                "account-dialog-cancel",
                "Cancel".into(),
                None,
                close.clone(),
                cx,
            ))
        })
        .when(naming || done, |foot| {
            let word: SharedString = if naming {
                format!("Continue with {label}").into()
            } else {
                "Done".into()
            };
            foot.child(
                div()
                    .when(naming && (busy || empty), |button| button.opacity(0.5))
                    .child(dialog::button(
                        "account-dialog-go",
                        word,
                        Some(t.accent),
                        go.clone(),
                        cx,
                    )),
            )
        });
    let body = div()
        .px(px(20.))
        .pt(px(8.))
        .text_size(crate::theme::ui_text(13.))
        .text_color(t.text_secondary)
        .child(body)
        .child(
            div()
                .pt(px(10.))
                .text_size(crate::theme::ui_text(11.5))
                .text_color(t.text_tertiary)
                .child("Each account keeps its sign-in in a folder of its own. Chats and settings are shared between them."),
        )
        .into_any_element();
    Some(dialog::sheet(title.into(), body, foot, go, close, cx).into_any_element())
}

/// The dialog's body while an account signs in, or after its sign-in failed.
fn signing_body(bench: &Workbench, id: &str, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .children(signing(bench, id, this, cx))
        .into_any_element()
}
