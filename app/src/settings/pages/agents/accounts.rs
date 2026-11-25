//! Each agent's accounts: who each is signed in as, which one is in use, and
//! the dialog that adds one through the agent's own login.

use crate::ui::Clicks;
use std::rc::Rc;

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, ClickEvent, Context, Div, Entity, IntoElement, ParentElement,
    SharedString, Stateful, Styled, Window,
};
use sikemux_agents::agent_accounts::{by_hand, Profile};

use crate::accounts::{self, SignIn};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, TEXT_2XS_H, TEXT_3XS_H, TEXT_XS,
};
use crate::ui::{self, dialog, sign_in};
use crate::workbench::Workbench;
use sikemux_agents::agent_accounts::BUILT_IN;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[crate::settings::index::Entry] = &[
    crate::settings::index::section(crate::settings::index::Page::Agents, "Accounts", "move between accounts smart rotation rotate switch limit week sign in login logout subscription max pro plus chatgpt add second work personal claude codex"),
];

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
        SignIn::Waiting { url, code, .. } => div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_2XS_H))
                    .text_color(t.text_tertiary)
                    .child(t!("Finish signing in in your browser")),
            )
            .children(url.clone().map(|url| {
                button(
                    "sign-in-page",
                    t!("Open page"),
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
                            t!("Send"),
                            Rc::new(move |bench, _, cx| accounts::submit_code(bench, &id, cx)),
                        )
                    }),
                None => row.child({
                    let id = owned.clone();
                    button(
                        "sign-in-code",
                        t!("Paste code"),
                        Rc::new(move |bench, window, cx| {
                            accounts::ask_code(bench, &id, window, cx)
                        }),
                    )
                }),
            })
            .child({
                let id = owned;
                button(
                    "sign-in-cancel",
                    t!("Cancel"),
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
                let id = owned;
                button(
                    "sign-in-retry",
                    t!("Try again"),
                    Rc::new(move |bench, window, cx| {
                        accounts::dismiss_failure(bench, &id);
                        accounts::sign_in(bench, &id, window, cx);
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
    let name = profile.shown_name(known);
    let signed_in = known.is_some_and(|status| status.signed_in);
    let who: Option<SharedString> = match status {
        None => Some(t!("Checking…").into()),
        Some(Ok(status)) if status.signed_in => status.plan.as_deref().map(|it| plan(it).into()),
        Some(Ok(_)) => Some(t!("Signed out").into()),
        Some(Err(_)) => None,
    };
    let id = profile.id.clone();
    let control: AnyElement = match signing(bench, &id, this, cx) {
        Some(signing) => signing,
        None if known.is_some() && !signed_in => {
            let (this, id) = (this.clone(), id.clone());
            ui::Button::new(
                SharedString::from(format!("account-sign-in-{id}")),
                t!("Sign in"),
                ui::Variant::Outline,
            )
            .on_click(move |_, window, cx| {
                let id = id.clone();
                this.update(cx, |bench, cx| accounts::sign_in(bench, &id, window, cx))
            })
            .into_any_element()
        }
        None if !in_use => {
            let (this, profile) = (this.clone(), profile.clone());
            ui::Button::new(
                SharedString::from(format!("account-use-{id}")),
                t!("Use"),
                ui::Variant::Outline,
            )
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| accounts::select(bench, &profile, cx))
            })
            .into_any_element()
        }
        None => div().into_any_element(),
    };
    let removable = !profile.id.starts_with(BUILT_IN);
    let book = bench.accounts.book.as_ref();
    let projects: Vec<(String, String, bool)> = (bench.projects().iter())
        .map(|project| {
            let folder = project.path.to_string_lossy().into_owned();
            let allowed = (book.and_then(|book| book.projects.get(&folder)))
                .is_none_or(|ids| ids.contains(&profile.id));
            (folder, project.name.clone(), allowed)
        })
        .collect();
    let itself = accounts::signs_in_itself(profile);
    let more = ((signed_in && !itself) || removable || !projects.is_empty()).then(|| {
        let (sign_out, remove, choose) = (this.clone(), this.clone(), this.clone());
        let (out_id, remove_id) = (id.clone(), id.clone());
        let remove_label: SharedString = t!("Remove {name}", name = name).into();
        ui::menu_button(
            SharedString::from(format!("account-more-{id}")),
            Anchor::TopRight,
            ui::IconButton::new(
                SharedString::from(format!("account-more-button-{id}")),
                Icon::More,
                t!("More for {name}", name = name),
            ),
            move |mut popup, _, _| {
                if signed_in && !itself {
                    let (sign_out, id) = (sign_out.clone(), out_id.clone());
                    popup = popup.item(
                        PopupMenuItem::new(t!("Sign out"))
                            .icon(Icon::LogOut)
                            .on_click(move |_, _, cx| {
                                sign_out.update(cx, |bench, cx| accounts::sign_out(bench, &id, cx))
                            }),
                    );
                }
                if !projects.is_empty() {
                    popup = popup.separator().label(t!("New chats here in"));
                }
                for (folder, name, allowed) in &projects {
                    let (choose, id, folder, allowed) =
                        (choose.clone(), out_id.clone(), folder.clone(), *allowed);
                    popup = popup.item(PopupMenuItem::new(name.clone()).checked(allowed).on_click(
                        move |_, _, cx| {
                            choose.update(cx, |bench, cx| {
                                accounts::set_in_project(bench, &id, &folder, !allowed, cx)
                            })
                        },
                    ));
                }
                if removable {
                    if !projects.is_empty() {
                        popup = popup.separator();
                    }
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
        .child(name)
        .when(in_use, |label| {
            label.child(
                div()
                    .px(px(5.))
                    .rounded(theme::radius_1())
                    .bg(t.surface_chosen)
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .text_color(t.text_secondary)
                    .child(t!("In use")),
            )
        });
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .py(SPACE_1)
        .child(accounts::avatar(bench.accounts.avatar(profile), 18.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(SPACE_2)
                .child(
                    div()
                        .text_size(crate::theme::ui_text(TEXT_XS))
                        .text_color(t.text_primary)
                        .child(label),
                )
                .children(who.map(|who| {
                    div()
                        .truncate()
                        .text_size(crate::theme::ui_text(TEXT_2XS_H))
                        .text_color(if known.is_some() && !signed_in {
                            t.warn
                        } else {
                            t.text_tertiary
                        })
                        .child(who)
                })),
        )
        .child(control)
        .children(more)
        .into_any_element()
}

/// The one switch for every agent: each new chat starts on the account whose week ends soonest,
/// and a chat at its limit carries on where the most is left.
pub fn smart(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let this = this.clone();
    let row = rows::row_with(
        None,
        t!("Move between accounts"),
        Some(t!("Starts each chat on the account whose week ends soonest and moves on at a limit. Antigravity's chats move only when you move them. Check each provider's terms first.").into()),
        Switch::new("smart-accounts")
            .checked(bench.accounts.smart())
            .on_change(move |on: &bool, _, cx| {
                let on = *on;
                this.update(cx, |bench, cx| accounts::set_smart(bench, on, cx))
            }),
        cx,
    );
    rows::section(n!("Accounts"), None, None, lit("Accounts"), row, cx)
}

/// An agent's accounts, one line each, and the way to add another.
pub fn list(bench: &Workbench, provider: &str, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let current = (bench.accounts.current(provider)).map(|profile| profile.id.clone());
    let add = {
        let (this, provider) = (this.clone(), provider.to_owned());
        ui::Button::new(
            SharedString::from(format!("account-add-{provider}")),
            t!("Add"),
            ui::Variant::Ghost,
        )
        .icon(Icon::Plus)
        .on_click(move |_, window, cx| {
            this.update(cx, |bench, cx| {
                accounts::open_dialog(bench, &provider, window, cx)
            })
        })
    };
    div()
        .flex()
        .flex_col()
        .children(bench.accounts.of(provider).iter().map(|profile| {
            let in_use = current.as_deref() == Some(profile.id.as_str());
            account_row(bench, profile, in_use, this, cx)
        }))
        .when(bench.accounts.book.is_none(), |body| {
            body.child(rows::note(t!("Reading accounts…"), cx))
        })
        .child(div().pt(SPACE_1).child(add))
        .into_any_element()
}

/// The add-account dialog while it is open, floating over whatever the window shows.
pub fn dialog(
    bench: &Workbench,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let screen = screen(bench, this, cx)?;
    let close: dialog::Answer = {
        let this = this.clone();
        Rc::new(move |_, cx| this.update(cx, accounts::close_dialog))
    };
    let go: dialog::Answer = {
        let this = this.clone();
        Rc::new(move |window, cx| this.update(cx, |bench, cx| submit(bench, window, cx)))
    };
    let body = div().px(SPACE_1).pt(SPACE_1).child(screen);
    Some(dialog::floating(px(440.), body, go, close).into_any_element())
}

/// Answers the add-account dialog as its main button does: adds the named account, hands on the
/// code typed, or closes the dialog once the account has signed in.
fn submit(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(open) = &bench.accounts.dialog else {
        return;
    };
    match open.account.clone() {
        None if !accounts::keeps_several(bench, &open.provider) => {
            accounts::close_dialog(bench, cx)
        }
        None => {
            let brand = brand(bench, &open.provider);
            accounts::add(bench, brand, window, cx);
        }
        Some(id) if !bench.accounts.signing.contains_key(&id) => accounts::close_dialog(bench, cx),
        Some(id) => accounts::submit_code(bench, &id, cx),
    }
}

fn brand(bench: &Workbench, provider: &str) -> String {
    bench
        .reads
        .catalog
        .kind(provider)
        .map_or("#8b8898", |kind| kind.brand.as_str())
        .to_owned()
}

/// What the add-account dialog shows: a name, then the agent's own login
/// in the browser, then who it signed in as.
fn screen(bench: &Workbench, this: &Entity<Workbench>, cx: &App) -> Option<sign_in::Screen> {
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
    let brand = brand(bench, &open.provider);
    let cancel = || {
        sign_in::links().child(
            sign_in::link("account-dialog-cancel", t!("Cancel"), cx)
                .clicks(on(this, |bench, _, cx| accounts::close_dialog(bench, cx))),
        )
    };
    let mark = ui::mark(&open.provider, 34., cx);
    let screen = match (account, signing) {
        (None, _) if !accounts::keeps_several(bench, &open.provider) => {
            one_sign_in(mark, &label, this)
        }
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
                        on(this, move |bench, window, cx| {
                            accounts::sign_in_again(bench, &id, window, cx)
                        })
                    };
                    div()
                        .flex()
                        .gap(SPACE_1H)
                        .child(
                            sign_in::choice(
                                SharedString::from(format!("sign-in-again-{id}")),
                                accounts::avatar(bench.accounts.avatar(profile), 18.),
                                profile.name.clone(),
                                t!("Signed out"),
                                cx,
                            )
                            .clicks(again),
                        )
                        .when(!profile.id.starts_with(BUILT_IN), |row| {
                            let remove = {
                                let id = id.clone();
                                on(this, move |bench, _, cx| accounts::remove(bench, &id, cx))
                            };
                            row.child(
                                sign_in::choice_beside(
                                    SharedString::from(format!("sign-in-remove-{id}")),
                                    Icon::Trash,
                                    t!("Remove {name}", name = profile.name),
                                    cx,
                                )
                                .clicks(remove),
                            )
                        })
                }))
            });
            sign_in::Screen::new(
                "account-dialog",
                mark,
                t!("Add an account for {agent}", agent = label),
            )
            .lede(t!(
                "Sign in with {label}'s own login in your browser.",
                label = label
            ))
            .child(sign_in::Field::new(t!("Name"), &open.name))
            .children((open.failed.clone()).map(|why| sign_in::error(why, cx)))
            .child(
                ui::Button::new(
                    "account-dialog-go",
                    match (open.busy, &open.failed) {
                        (true, _) => t!("Adding account…").to_owned(),
                        (false, Some(_)) => t!("Try again").to_owned(),
                        (false, None) => t!("Continue with {label}", label = label),
                    },
                    ui::Variant::Primary,
                )
                .wide()
                .disabled(open.busy || empty)
                .on_click(on(this, move |bench, window, cx| {
                    accounts::add(bench, brand.clone(), window, cx)
                })),
            )
            .when_some(again, |screen, again| {
                screen
                    .child(sign_in::or(t!("or sign in again"), cx))
                    .child(again)
            })
            .child(cancel())
        }
        (Some(profile), Some(SignIn::Waiting { url, by_code, .. }))
            if by_hand(&profile.provider) && (*by_code || url.is_some()) =>
        {
            pasted_sign_in(profile, url.clone(), open, mark, &label, this, cx)
        }
        (Some(profile), Some(SignIn::Waiting { url, code, .. })) => {
            let id = profile.id.clone();
            let by_code = (by_hand(&profile.provider))
                .then(|| sign_in::links().child(with_code(profile, this, cx)));
            let help = url.clone().map(|url| match code {
                Some(field) => {
                    let id = id.clone();
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(SPACE_3)
                        .child(sign_in::Field::new(t!("Code from the page"), field).mono())
                        .child(
                            ui::Button::new(
                                "sign-in-send-code",
                                t!("Send code"),
                                ui::Variant::Outline,
                            )
                            .wide()
                            .on_click(on(this, move |bench, _, cx| {
                                accounts::submit_code(bench, &id, cx)
                            })),
                        )
                }
                None => sign_in::links()
                    .child(
                        sign_in::link("sign-in-page", t!("Open the page again"), cx)
                            .clicks(move |_, _, cx| cx.open_url(&url)),
                    )
                    .child(
                        sign_in::link("sign-in-code", t!("Paste a code"), cx)
                            .clicks(on(this, move |bench, window, cx| {
                                accounts::ask_code(bench, &id, window, cx)
                            })),
                    ),
            });
            sign_in::Screen::new(
                "account-dialog",
                mark,
                t!("Sign in to {name}", name = profile.name),
            )
            .lede(t!(
                "Sign in with {label}'s own login in your browser.",
                label = label
            ))
            .child(sign_in::waiting(
                "account-dialog-waiting",
                t!("Finish signing in in your browser"),
                on(this, |bench, _, cx| accounts::close_dialog(bench, cx)),
                cx,
            ))
            .children(help)
            .children(by_code)
        }
        (Some(profile), Some(SignIn::Failed(message))) => {
            let id = profile.id.clone();
            sign_in::Screen::new(
                "account-dialog",
                mark,
                t!("Sign in to {name}", name = profile.name),
            )
            .lede(t!(
                "Sign in with {label}'s own login in your browser.",
                label = label
            ))
            .child(sign_in::error(message.clone(), cx))
            .child(
                ui::Button::new("sign-in-retry", t!("Try again"), ui::Variant::Primary)
                    .wide()
                    .on_click(on(this, move |bench, window, cx| {
                        accounts::sign_in_again(bench, &id, window, cx);
                    })),
            )
            .child(cancel().when(by_hand(&profile.provider), |links| {
                links.child(with_code(profile, this, cx))
            }))
        }
        (Some(profile), None) => signed_in(bench, profile, mark, this),
    };
    Some(screen)
}

/// Says an agent has one sign-in, in place of the form that would add a second.
fn one_sign_in(mark: AnyElement, label: &str, this: &Entity<Workbench>) -> sign_in::Screen {
    sign_in::Screen::new(
        "account-dialog",
        mark,
        t!("{agent} has one account", agent = label),
    )
    .lede(t!(
        "Sikemux runs {agent} on its one sign-in, so there is no second account to add. To use another, sign out with {agent}'s own command in a terminal, then sign in again.",
        agent = label
    ))
    .child(
        ui::Button::new("account-dialog-close", t!("Close"), ui::Variant::Primary)
            .wide()
            .on_click(on(this, |bench, _, cx| accounts::close_dialog(bench, cx))),
    )
}

fn with_code(profile: &Profile, this: &Entity<Workbench>, cx: &App) -> Stateful<Div> {
    let id = profile.id.clone();
    sign_in::link("sign-in-by-code", t!("Use a code instead"), cx)
        .clicks(on(this, move |bench, window, cx| {
            accounts::sign_in_by_code(bench, &id, window, cx)
        }))
}

/// A sign-in whose page, which its agent opens, shows a code to paste here. The field is there as
/// the page opens, so the person has one page and one code.
fn pasted_sign_in(
    profile: &Profile,
    url: Option<String>,
    open: &accounts::Dialog,
    mark: AnyElement,
    label: &str,
    this: &Entity<Workbench>,
    cx: &App,
) -> sign_in::Screen {
    let close = on(this, |bench, _, cx| accounts::close_dialog(bench, cx));
    let id = profile.id.clone();
    let typed = open.code.read(cx).value().trim().to_owned();
    let sent = open.sent.as_ref() == Some(&typed);
    let open_page = url.clone();
    let action = match (&url, typed.is_empty()) {
        (None, _) => ui::Button::new(
            "sign-in-send-code",
            t!("Opening {label}'s page…", label = label),
            ui::Variant::Primary,
        )
        .disabled(true),
        (Some(_), true) => ui::Button::new(
            "sign-in-paste-code",
            t!("Paste the code"),
            ui::Variant::Primary,
        )
        .on_click(on(this, move |bench, window, cx| {
            accounts::paste_code(bench, &id, window, cx)
        })),
        (Some(_), false) => ui::Button::new(
            "sign-in-send-code",
            match sent {
                true => t!("Signing in…"),
                false => t!("Sign in"),
            },
            ui::Variant::Primary,
        )
        .disabled(sent)
        .on_click(on(this, move |bench, _, cx| {
            accounts::submit_code(bench, &id, cx)
        })),
    };
    sign_in::Screen::new(
        "account-dialog",
        mark,
        t!("Sign in to {name}", name = profile.name),
    )
    .lede(t!(
        "Sign in with {label}'s page in your browser, then paste the code it shows.",
        label = label
    ))
    .child(sign_in::Field::new(t!("Code from the page"), &open.code).mono())
    .child(action.wide())
    .child(
        sign_in::links()
            .children(open_page.map(|url| {
                sign_in::link("sign-in-page", t!("Open the page again"), cx)
                    .clicks(move |_, _, cx| cx.open_url(&url))
            }))
            .child(sign_in::link("account-dialog-cancel", t!("Cancel"), cx).clicks(close)),
    )
}

/// Who the account signed in as, with the way to start using it while another is in use.
fn signed_in(
    bench: &Workbench,
    profile: &Profile,
    mark: AnyElement,
    this: &Entity<Workbench>,
) -> sign_in::Screen {
    let status = accounts::status(bench, profile).and_then(|status| status.as_ref().ok());
    let named = profile.shown_name(status);
    let lede = match status {
        None => t!("Checking who signed in…").to_owned(),
        Some(status) if !status.signed_in => t!("It is not signed in yet.").to_owned(),
        Some(status) => [
            status.email.clone().or_else(|| status.organization.clone()),
            status.plan.as_deref().map(plan),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · "),
    };
    let screen = sign_in::Screen::new(
        "account-dialog",
        mark,
        t!("{named} is signed in", named = named),
    )
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
                        t!("Switch to {named}", named = named),
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
                        t!(
                            "Keep using {name}",
                            name = current.shown_name(current_status)
                        ),
                        ui::Variant::Outline,
                    )
                    .wide()
                    .on_click(close()),
                )
        }
        None => screen.child(
            ui::Button::new("account-dialog-done", t!("Done"), ui::Variant::Primary)
                .wide()
                .on_click(close()),
        ),
    }
}

fn on(
    this: &Entity<Workbench>,
    run: impl Fn(&mut Workbench, &mut Window, &mut Context<Workbench>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let this = this.clone();
    move |_, window, cx| this.update(cx, |bench, cx| run(bench, window, cx))
}
