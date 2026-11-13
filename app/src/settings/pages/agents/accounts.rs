//! Each agent's accounts: who each is signed in as, which one is in use, and
//! the dialog that adds one through the agent's own login.

use crate::ui::Clicks;
use std::rc::Rc;

use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, ClickEvent, Context, Entity, IntoElement, ParentElement,
    SharedString, Styled, Window,
};
use sikemux_agents::agent_accounts::Profile;

use crate::accounts::{self, SignIn};
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS_H,
    TEXT_3XS_H, TEXT_SM,
};
use crate::ui::{self, dialog, sign_in};
use crate::workbench::Workbench;
use sikemux_agents::agent_accounts::BUILT_IN;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[crate::settings::index::Entry] = &[
    crate::settings::index::section(crate::settings::index::Page::Accounts, "Accounts", "profiles executable path sign in login logout email subscription max pro plus chatgpt switch rotate limit add second work personal claude codex"),
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
        SignIn::Waiting { url, code } => div()
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
    let name = profile.shown_name(known);
    let signed_in = known.is_some_and(|status| status.signed_in);
    let who: SharedString = match status {
        None => t!("Checking…").into(),
        Some(Err(_)) => t!("Could not ask who is signed in").into(),
        Some(Ok(status)) if status.signed_in => {
            let parts: Vec<String> = [
                status.email.clone().or_else(|| status.organization.clone()),
                status.plan.as_deref().map(plan),
            ]
            .into_iter()
            .flatten()
            .collect();
            match parts.is_empty() {
                true => t!("Signed in").into(),
                false => parts.join(" · ").into(),
            }
        }
        Some(Ok(_)) => t!("Signed out").into(),
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
    let more = (signed_in || removable || !projects.is_empty()).then(|| {
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
                if signed_in {
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
                    .rounded(theme::RADIUS_1)
                    .bg(t.surface_chosen)
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .text_color(t.text_secondary)
                    .child(t!("In use")),
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
        .child(accounts::avatar(bench.accounts.avatar(profile), 24.))
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
        t!("Smart accounts"),
        Some(t!("Starts each chat on the account whose week ends soonest and moves on at a limit. Check each provider's terms first.").into()),
        Switch::new("smart-accounts")
            .checked(bench.accounts.smart())
            .on_change(move |on: &bool, _, cx| {
                let on = *on;
                this.update(cx, |bench, cx| accounts::set_smart(bench, on, cx))
            }),
        cx,
    );
    rows::section(n!("Rotation"), None, None, lit("Rotation"), row, cx)
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
    let add = {
        let (this, provider) = (this.clone(), provider.to_owned());
        div().p(SPACE_2H).child(
            ui::Button::new(
                SharedString::from(format!("account-add-{provider}")),
                t!("Add account"),
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
        .child(add)
        .when(bench.accounts.book.is_none(), |body| {
            body.child(rows::note(t!("Reading accounts…"), cx))
        });
    rows::section(
        title,
        Some(match count {
            1 => t!("1 account").into(),
            count => t!("{count} accounts", count = count).into(),
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
            sign_in::link("account-dialog-cancel", t!("Cancel"), cx)
                .clicks(on(this, |bench, _, cx| accounts::close_dialog(bench, cx))),
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
                t!("Add a {label} account", label = label),
            )
            .lede(t!(
                "Sign in with {label}'s own login in your browser.",
                label = label
            ))
            .foot(crate::i18n::tr(FOOT))
            .child(sign_in::Field::new(t!("Name"), &open.name))
            .child(
                ui::Button::new(
                    "account-dialog-go",
                    t!("Continue with {label}", label = label),
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
                    .child(sign_in::or(t!("or sign in again"), cx))
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
            .foot(crate::i18n::tr(FOOT))
            .child(sign_in::waiting(
                "account-dialog-waiting",
                t!("Finish signing in in your browser"),
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
                t!("Sign in to {name}", name = profile.name),
            )
            .lede(t!(
                "Sign in with {label}'s own login in your browser.",
                label = label
            ))
            .foot(crate::i18n::tr(FOOT))
            .child(sign_in::error(message.clone(), cx))
            .child(
                ui::Button::new("sign-in-retry", t!("Try again"), ui::Variant::Primary)
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
    n!("Each account keeps its sign-in in a folder of its own. Chats and settings are shared between them.");

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
