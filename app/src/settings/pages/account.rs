//! The account this host is signed in to.

use gpui_kit::{
    div, px, AnyElement, App, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_wire::accounts::protocol::RevokeReason;
use sikemux_wire::protocol::{AccountLink, AccountLinkState};

use crate::settings::index::Page::Account;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{Tokens, SPACE_1, SPACE_2, SPACE_4, TEXT_MD, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[section(
    Account,
    "Account",
    "account sign in sign out login google github email delete",
)];

const DELETE_ACCOUNT_URL: &str = "https://app.sikemux.com/delete-account";

fn account_line(link: Option<&AccountLink>) -> &'static str {
    match link.map(|link| link.state) {
        Some(AccountLinkState::Connecting) => "Connecting to your account",
        Some(AccountLinkState::Offline) => "Can't reach your account, retrying",
        _ => "Signed in to Sikemux",
    }
}

/// Why this host is signed out, when the account let it go rather than the person here.
fn removal_note(link: Option<&AccountLink>) -> Option<&'static str> {
    let link = link.filter(|link| link.state == AccountLinkState::Removed)?;
    Some(match link.reason {
        Some(RevokeReason::AccountDeleted) => {
            "Your account was deleted. Phones already allowed stay allowed."
        }
        Some(RevokeReason::SignedOut) => {
            "This host was signed out of your account. Phones already allowed stay allowed."
        }
        _ => "This host was removed from your account at app.sikemux.com. Phones already allowed stay allowed.",
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
            "Finish signing in in your browser",
            "Sikemux opened the sign-in page in your default browser.",
        ),
        false => (
            "Sign in to your Sikemux account",
            removal_note(link).unwrap_or(
                "Your phone finds this host through it. Sign in with Google, GitHub or your email, in your browser.",
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
            .on_click(move |_, _, cx| this.update(cx, |bench, _| bench.sign_in())),
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
            "Your account",
            "Checking…",
            div().into_any_element(),
            cx,
        ),
        None => sign_in_panel(bench, link, this, cx),
        Some(account) => {
            let (delete, leave) = (this.clone(), this.clone());
            top_row(
                crate::rail::avatar(bench, account, 28., &t).into_any_element(),
                account
                    .email
                    .clone()
                    .or_else(|| account.name.clone())
                    .unwrap_or_else(|| "Signed in".into()),
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
    vec![rows::section(
        t!("Account"),
        None,
        None,
        lit("Account"),
        body,
        cx,
    )]
}
