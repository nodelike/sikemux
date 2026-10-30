//! Each agent's accounts: who each is signed in as, which one is in use, and
//! the dialog that adds one through the agent's own login.

use std::rc::Rc;

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window,
};
use sikemux_core::agent_accounts::Profile;

use crate::accounts::{self, SignIn, Status};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H,
    TEXT_3XS_H, TEXT_SM,
};
use crate::ui::{self, dialog, sign_in};
use crate::workbench::Workbench;

/// What the account is called: a built-in one still under its agent's name goes by
/// the name on the account instead.
fn name(bench: &Workbench, profile: &Profile, status: Option<&Status>) -> String {
    let unnamed =
        profile.id.starts_with("builtin-") && profile.name == bench.agent_name(&profile.provider);
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
            .gap(SPACE_2)
            .child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
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
            .gap(SPACE_2)
            .child(
                div()
                    .max_w(px(240.))
                    .truncate()
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
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
        ui::menu_button(
            SharedString::from(format!("account-more-{id}")),
            Anchor::TopRight,
            ui::IconButton::new(
                SharedString::from(format!("account-more-button-{id}")),
                Icon::More,
                format!("More for {name}"),
            ),
            move |mut popup, _, _| {
                if signed_in {
                    let (sign_out, id) = (sign_out.clone(), out_id.clone());
                    popup = popup.item(PopupMenuItem::new("Sign out").icon(Icon::LogOut).on_click(
                        move |_, _, cx| {
                            sign_out.update(cx, |bench, cx| accounts::sign_out(bench, &id, cx))
                        },
                    ));
                }
                if removable {
                    let (remove, id) = (remove.clone(), remove_id.clone());
                    popup = popup.item(
                        PopupMenuItem::new(remove_label.clone())
                            .icon(Icon::Trash)
                            .on_click(move |_, _, cx| {
                                remove.update(cx, |bench, cx| accounts::remove(bench, &id, cx))
                            }),
                    );
                }
                popup
            },
        )
    });
    let label = div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .child(name.clone())
        .when(in_use, |label| {
            label.child(
                div()
                    .px(px(5.))
                    .rounded(theme::RADIUS_1)
                    .bg(t.surface_chosen)
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .text_color(t.text_secondary)
                    .child("In use"),
            )
        });
    div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .px(SPACE_4)
        .py(SPACE_2H)
        .border_b_1()
        .border_color(t.line_soft)
        .child(accounts::avatar(profile, &name, 24.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(SPACE_0)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_SM))
                        .text_color(t.text_primary)
                        .child(label),
                )
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
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
    provider: &str,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let title = bench.agent_name(provider).to_owned();
    let list = bench.accounts.of(provider);
    let current = bench
        .accounts
        .current(provider)
        .map(|profile| profile.id.clone());
    let count = list.len();
    let auto = (count > 1).then(|| {
        let (this, provider) = (this.clone(), provider.to_owned());
        rows::row_with(
            None,
            "Auto-rotate at limits",
            Some(format!("Moves to the next account when one hits its limit. Check {title}'s terms first.").into()),
            Switch::new(SharedString::from(format!("auto-rotate-{provider}")))
                .checked(bench.accounts.auto_switch(&provider))
                .on_change(move |on: &bool, _, cx| {
                    let on = *on;
                    this.update(cx, |bench, cx| {
                        accounts::set_auto_switch(bench, &provider, on, cx)
                    })
                }),
            cx,
        )
    });
    let add = {
        let (this, provider) = (this.clone(), provider.to_owned());
        div().p(SPACE_2H).child(
            ui::Button::new(
                SharedString::from(format!("account-add-{provider}")),
                "Add account",
                ui::Variant::Dashed,
            )
            .icon(Icon::Plus)
            .on_click(move |_, window, cx| {
                this.update(cx, |bench, cx| {
                    accounts::open_dialog(bench, &provider, window, cx)
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
        title.clone(),
        Some(match count {
            1 => "1 account".into(),
            count => format!("{count} accounts").into(),
        }),
        None,
        lit("Accounts"),
        body,
        cx,
    )
}

/// The add-account dialog while it is open: a name, then the agent's own login in the
/// browser, then who it signed in as.
pub fn dialog(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let open = bench.accounts.dialog.as_ref()?;
    let label = bench.agent_name(&open.provider).to_owned();
    let account = open
        .account
        .as_deref()
        .and_then(|id| bench.accounts.profile(id));
    let signing = open
        .account
        .as_deref()
        .and_then(|id| bench.accounts.signing.get(id));
    let brand = bench
        .reads
        .catalog
        .kind(&open.provider)
        .map_or("#8b8898", |kind| kind.brand.as_str())
        .to_owned();
    let close: dialog::Answer = {
        let this = this.clone();
        Rc::new(move |_, cx| this.update(cx, accounts::close_dialog))
    };
    let cancel = || {
        sign_in::links().child(
            sign_in::link("account-dialog-cancel", "Cancel", cx)
                .on_click(on(this, |bench, _, cx| accounts::close_dialog(bench, cx))),
        )
    };
    let mark = ui::mark(&open.provider, 34.);
    let screen = match (account, signing) {
        (None, _) => {
            let brand = brand.clone();
            let empty = open.name.read(cx).value().trim().is_empty();
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
                sign_in::choices().children(signed_out.into_iter().map(|profile| {
                    let id = profile.id.clone();
                    let again = {
                        let id = id.clone();
                        on(this, move |bench, _, cx| {
                            accounts::sign_in_again(bench, &id, cx)
                        })
                    };
                    div()
                        .flex()
                        .gap(SPACE_1H)
                        .child(
                            sign_in::choice(
                                SharedString::from(format!("sign-in-again-{id}")),
                                accounts::avatar(profile, &profile.name, 18.),
                                profile.name.clone(),
                                "Signed out",
                                cx,
                            )
                            .on_click(again),
                        )
                        .when(!profile.id.starts_with("builtin-"), |row| {
                            let remove = {
                                let id = id.clone();
                                on(this, move |bench, _, cx| accounts::remove(bench, &id, cx))
                            };
                            row.child(
                                sign_in::choice_beside(
                                    SharedString::from(format!("sign-in-remove-{id}")),
                                    Icon::Trash,
                                    format!("Remove {}", profile.name),
                                    cx,
                                )
                                .on_click(remove),
                            )
                        })
                }))
            });
            sign_in::Screen::new("account-dialog", mark, format!("Add a {label} account"))
                .lede(format!("Sign in with {label}'s own login in your browser."))
                .foot(FOOT)
                .child(sign_in::Field::new("Name", &open.name))
                .child(
                    ui::Button::new(
                        "account-dialog-go",
                        format!("Continue with {label}"),
                        ui::Variant::Primary,
                    )
                    .wide()
                    .disabled(open.busy || empty)
                    .on_click(on(this, move |bench, _, cx| {
                        accounts::add(bench, brand.clone(), cx)
                    })),
                )
                .when_some(again, |screen, again| {
                    screen
                        .child(sign_in::or("or sign in again", cx))
                        .child(again)
                })
                .child(cancel())
        }
        (Some(profile), Some(SignIn::Waiting { url, code })) => {
            let id = profile.id.clone();
            let help = url.clone().map(|url| match code {
                Some(field) => {
                    let id = id.clone();
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(SPACE_3)
                        .child(sign_in::Field::new("Code from the page", field).mono())
                        .child(
                            ui::Button::new("sign-in-send-code", "Send code", ui::Variant::Outline)
                                .wide()
                                .on_click(on(this, move |bench, _, cx| {
                                    accounts::submit_code(bench, &id, cx)
                                })),
                        )
                }
                None => sign_in::links()
                    .child(
                        sign_in::link("sign-in-page", "Open the page again", cx)
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                    .child(
                        sign_in::link("sign-in-code", "Paste a code", cx)
                            .on_click(on(this, move |bench, window, cx| {
                                accounts::ask_code(bench, &id, window, cx)
                            })),
                    ),
            });
            sign_in::Screen::new(
                "account-dialog",
                mark,
                format!("Sign in to {}", profile.name),
            )
            .lede(format!("Sign in with {label}'s own login in your browser."))
            .foot(FOOT)
            .child(sign_in::waiting(
                "account-dialog-waiting",
                "Finish signing in in your browser",
                on(this, |bench, _, cx| accounts::close_dialog(bench, cx)),
                cx,
            ))
            .children(help)
        }
        (Some(profile), Some(SignIn::Failed(message))) => {
            let id = profile.id.clone();
            sign_in::Screen::new(
                "account-dialog",
                mark,
                format!("Sign in to {}", profile.name),
            )
            .lede(format!("Sign in with {label}'s own login in your browser."))
            .foot(FOOT)
            .child(sign_in::error(message.clone(), cx))
            .child(
                ui::Button::new("sign-in-retry", "Try again", ui::Variant::Primary)
                    .wide()
                    .on_click(on(this, move |bench, _, cx| {
                        accounts::dismiss_failure(bench, &id);
                        accounts::sign_in(bench, &id, cx);
                    })),
            )
            .child(cancel())
        }
        (Some(profile), None) => signed_in(bench, profile, mark, this),
    };
    let go: dialog::Answer = {
        let this = this.clone();
        let naming = account.is_none();
        let done = account.is_some() && signing.is_none();
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
    let body = div().px(SPACE_1).pt(SPACE_1).child(screen);
    Some(dialog::floating(px(440.), body, go, close, cx).into_any_element())
}

const FOOT: &str =
    "Each account keeps its sign-in in a folder of its own. Chats and settings are shared between them.";

/// Who the account signed in as, with the way to start using it while another is in use.
fn signed_in(
    bench: &Workbench,
    profile: &Profile,
    mark: AnyElement,
    this: &Entity<Workbench>,
) -> sign_in::Screen {
    let status = accounts::status(bench, profile).and_then(|status| status.as_ref().ok());
    let named = name(bench, profile, status);
    let lede = match status {
        None => "Checking who signed in…".to_owned(),
        Some(status) if !status.signed_in => "It is not signed in yet.".to_owned(),
        Some(status) => [
            status.email.clone().or_else(|| status.organization.clone()),
            status.plan.as_deref().map(plan),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
    };
    let screen = sign_in::Screen::new("account-dialog", mark, format!("{named} is signed in"))
        .when(!lede.is_empty(), |screen| screen.lede(lede));
    let close = || on(this, |bench, _, cx| accounts::close_dialog(bench, cx));
    match bench
        .accounts
        .current(&profile.provider)
        .filter(|current| current.id != profile.id)
    {
        Some(current) => {
            let current_status =
                accounts::status(bench, current).and_then(|status| status.as_ref().ok());
            let switch_to = profile.clone();
            screen
                .child(
                    ui::Button::new(
                        "account-dialog-switch",
                        format!("Switch to {named}"),
                        ui::Variant::Primary,
                    )
                    .wide()
                    .on_click(on(this, move |bench, _, cx| {
                        accounts::select(bench, &switch_to, cx);
                        accounts::close_dialog(bench, cx);
                    })),
                )
                .child(
                    ui::Button::new(
                        "account-dialog-keep",
                        format!("Keep using {}", name(bench, current, current_status)),
                        ui::Variant::Outline,
                    )
                    .wide()
                    .on_click(close()),
                )
        }
        None => screen.child(
            ui::Button::new("account-dialog-done", "Done", ui::Variant::Primary)
                .wide()
                .on_click(close()),
        ),
    }
}

/// A click that runs `run` on the workbench.
fn on(
    this: &Entity<Workbench>,
    run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let this = this.clone();
    move |_, window, cx| this.update(cx, |bench, cx| run(bench, window, cx))
}
