//! The account this host is signed in to.

use gpui_kit::{
    div, px, AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_corekit::accounts;
use sikemux_view::device::{AccountOrg, OrgRole};
use sikemux_wire::accounts::protocol::RevokeReason;
use sikemux_wire::protocol::{AccountLink, AccountLinkState};

use crate::icons::Icon;
use crate::settings::index::Page::Account;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_1, SPACE_2, SPACE_4, TEXT_2XS_H, TEXT_MD, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(
        Account,
        "Account",
        "account sign in sign out login apple google github microsoft email delete",
    ),
    section(
        Account,
        "Orgs",
        "orgs org team teams groups plan billing members",
    ),
];

const DELETE_ACCOUNT_URL: &str = "https://app.sikemux.com/delete-account";

fn account_line(link: Option<&AccountLink>) -> &'static str {
    match link.map(|link| link.state) {
        Some(AccountLinkState::Connecting) => t!("Connecting to your account"),
        Some(AccountLinkState::Offline) => t!("Can't reach your account, retrying"),
        _ => t!("Signed in to Sikemux"),
    }
}

/// Why this host is signed out, when the account let it go rather than the person here.
fn removal_note(link: Option<&AccountLink>) -> Option<&'static str> {
    let link = link.filter(|link| link.state == AccountLinkState::Removed)?;
    Some(match link.reason {
        Some(RevokeReason::AccountDeleted) => {
            t!("Your account was deleted. Phones already allowed stay allowed.")
        }
        Some(RevokeReason::SignedOut) => {
            t!("This host was signed out of your account. Phones already allowed stay allowed.")
        }
        _ => t!("This host was removed from your account at app.sikemux.com. Phones already allowed stay allowed."),
    })
}

/// A row at the top of the page: a mark, what it is, a line about it, and its control.
fn top_row(
    mark: AnyElement,
    title: impl Into<SharedString>,
    about: impl Into<SharedString>,
    control: AnyElement,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    rows::row_with(
        Some(
            ui::mark_box(28., mark)
                .text_color(t.text_secondary)
                .into_any_element(),
        ),
        title,
        Some(about.into()),
        control,
        cx,
    )
    .into_any_element()
}

fn sign_in_panel(
    bench: &Workbench,
    link: Option<&AccountLink>,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let waiting = bench.signing_in;
    let (title, lede) = match waiting {
        true => (
            t!("Finish signing in in your browser"),
            t!("Sikemux opened the sign-in page in your default browser."),
        ),
        false => (
            t!("Sign in to your Sikemux account"),
            removal_note(link).unwrap_or(
                t!("Your phone finds this host through it. Sign in with Apple, Google, GitHub, Microsoft or your email, in your browser."),
            ),
        ),
    };
    let this = this.clone();
    let action = match waiting {
        true => ui::Button::new("devices-cancel-sign-in", t!("Cancel"), ui::Variant::Outline)
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    bench.cancel_sign_in();
                    cx.notify();
                })
            }),
        false => ui::Button::new("devices-sign-in", t!("Sign in"), ui::Variant::Primary)
            .on_click(move |_, _, cx| this.update(cx, |bench, cx| bench.sign_in(cx))),
    };
    div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .p(SPACE_4)
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_MD))
                .text_color(t.text_primary)
                .child(title),
        )
        .child(
            div()
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(lede),
        )
        .child(div().pt(SPACE_1).flex().child(action))
        .into_any_element()
}

fn role_name(role: OrgRole) -> &'static str {
    match role {
        OrgRole::Owner => t!("Owner"),
        OrgRole::Admin => t!("Admin"),
        OrgRole::User => t!("Member"),
    }
}

fn plan_state(org: &AccountOrg, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let (mark, label, color) = match org.active {
        true => (
            ui::dot(ui::Tone::Ok, cx).into_any_element(),
            match org.preview {
                true => t!("Free during preview"),
                false => t!("Active"),
            },
            t.text_secondary,
        ),
        false => (
            ui::plan_lapsed_mark(10., cx).into_any_element(),
            t!("Plan lapsed"),
            t.warn,
        ),
    };
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .text_size(crate::theme::ui_text(TEXT_2XS_H))
        .text_color(color)
        .child(mark)
        .child(label)
        .into_any_element()
}

/// The orgs the account is in, each with its role and whether its plan runs.
fn orgs_section(orgs: &[AccountOrg], lit: bool, this: &Entity<Workbench>, cx: &App) -> AnyElement {
    let this = this.clone();
    let open = ui::Button::new("account-orgs-open", t!("Open"), ui::Variant::Outline)
        .icon(Icon::ExternalLink)
        .on_click(move |_, _, cx| {
            this.update(cx, |_, cx| {
                cx.open_url(&format!("{}/team", accounts::app_base()))
            })
        });
    let body = div()
        .flex()
        .flex_col()
        .children(orgs.iter().map(|org| {
            rows::row_with(
                Some(ui::org_mark(&org.name, &org.id, 24.).into_any_element()),
                org.name.clone(),
                Some(role_name(org.role).into()),
                plan_state(org, cx),
                cx,
            )
        }))
        .child(rows::row_keeping_control(
            None,
            t!("People, groups, billing and each org's policy are on app.sikemux.com"),
            None,
            open,
            cx,
        ));
    rows::section(
        n!("Orgs"),
        None,
        Some(t!("The orgs you're in. Their model keys and skills show beside your own.").into()),
        lit,
        body,
        cx,
    )
}

pub fn render(
    bench: &Workbench,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let t = Tokens::get(cx).clone();
    let known = bench.this_view().is_some();
    let account = bench.this_view().and_then(|view| view.account.as_deref());
    let link = (bench.remote.as_ref()).and_then(|status| status.account.as_ref());
    let body: AnyElement = match account {
        None if !known => top_row(
            div()
                .size(px(28.))
                .rounded_full()
                .bg(t.surface_raised)
                .into_any_element(),
            t!("Your account"),
            t!("Checking…"),
            div().into_any_element(),
            cx,
        ),
        None => sign_in_panel(bench, link, this, cx),
        Some(account) => {
            let (delete, leave) = (this.clone(), this.clone());
            top_row(
                crate::rail::avatar(bench, account, 28., &t).into_any_element(),
                account
                    .name
                    .clone()
                    .unwrap_or_else(|| t!("Your account").into()),
                account_line(link),
                div()
                    .flex()
                    .gap(SPACE_2)
                    .child(
                        ui::Button::new(
                            "account-delete",
                            t!("Delete account…"),
                            ui::Variant::Outline,
                        )
                        .on_click(move |_, _, cx| {
                            delete.update(cx, |_, cx| cx.open_url(DELETE_ACCOUNT_URL))
                        }),
                    )
                    .child(
                        ui::Button::new("account-sign-out", t!("Sign out"), ui::Variant::Outline)
                            .on_click(move |_, _, cx| {
                                leave.update(cx, |bench, _| bench.sign_out())
                            }),
                    )
                    .into_any_element(),
                cx,
            )
        }
    };
    let orgs: Vec<_> = accounts::shown_orgs(account).cloned().collect();
    let mut sections = vec![rows::section(
        n!("Account"),
        None,
        None,
        lit("Account"),
        body,
        cx,
    )];
    if !orgs.is_empty() {
        sections.push(orgs_section(&orgs, lit("Orgs"), this, cx));
    }
    sections
}
