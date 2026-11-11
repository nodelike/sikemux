//! Which of the person's accounts with its agent a chat runs on. One that has started moves
//! there, even in the middle of a turn, to the accounts the core says can take it; one that has
//! not simply starts there, on any signed in.

use std::sync::Arc;

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::Disableable;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, Context, Image, IntoElement, ParentElement, SharedString,
    Styled,
};
use sikemux_core::agent_accounts::status::AccountStatus;
use sikemux_core::agent_accounts::Profile;
use std::rc::Rc;

use crate::accounts;
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1H, SPACE_2, TEXT_2XS};
use crate::ui;
use crate::workbench::Workbench;

/// What picking an account does.
pub type Pick = Rc<dyn Fn(&Profile, &mut App)>;

/// What picks an account: what keeps its picker apart from others, which agent it is, the
/// account it runs on, the chat once it has started, and what picking one does.
pub struct Chat<'a> {
    pub key: &'a str,
    pub provider: &'a str,
    pub on: &'a str,
    pub started: Option<&'a str>,
    pub pick: Pick,
}

/// The picker for the chat, or none while its agent has only the one account. When no other
/// account can take the chat, its menu says why.
pub fn picker(
    bench: &Workbench,
    chat: Chat,
    running: bool,
    disabled: bool,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let Chat {
        key,
        provider,
        on,
        started,
        pick,
    } = chat;
    let t = Tokens::get(cx).clone();
    let current = bench.accounts.profile(on)?.clone();
    if bench.accounts.of(provider).len() < 2 {
        return None;
    }
    let signed_in = |profile: &Profile| {
        accounts::status(bench, profile)
            .and_then(|status| status.as_ref().ok())
            .filter(|status| status.signed_in)
            .cloned()
    };
    let (movable, none_because): (Vec<(Profile, AccountStatus)>, Option<String>) = match started {
        Some(agent) => {
            let takers = accounts::takers(bench, agent, on, cx).cloned();
            let others = takers.as_ref().map(|takers| &takers.others);
            let movable = (bench.accounts.of(provider).into_iter())
                .filter(|profile| {
                    profile.id == current.id
                        || others.is_some_and(|others| others.iter().any(|o| o.id == profile.id))
                })
                .filter_map(|profile| Some((profile.clone(), signed_in(profile)?)))
                .collect();
            (movable, takers.and_then(|takers| takers.none_because))
        }
        None => {
            let movable: Vec<_> = (bench.accounts.of(provider).into_iter())
                .filter_map(|profile| Some((profile.clone(), signed_in(profile)?)))
                .collect();
            let alone = !movable.iter().any(|(profile, _)| profile.id != current.id);
            (
                movable,
                alone.then(|| "No other account is signed in".to_owned()),
            )
        }
    };
    let name = |profile: &Profile, status: Option<&AccountStatus>| profile.shown_name(status);
    let label = name(&current, signed_in(&current).as_ref());
    let about = match (&none_because, running) {
        (Some(why), _) => why.clone(),
        (None, true) => "Moving stops this turn and carries it on in the other account".to_owned(),
        (None, false) => label.clone(),
    };
    let trigger = KitButton::new(SharedString::from(format!("account-picker-{key}")))
        .ghost()
        .compact()
        .disabled(disabled)
        .h(px(26.))
        .max_w(px(200.))
        .px(SPACE_2)
        .gap(SPACE_1H)
        .rounded_full()
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .text_color(t.text_secondary)
        .tooltip(about)
        .child(accounts::avatar(bench.accounts.avatar(&current), 16.))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(label.clone()),
        )
        .child(ui::icon(Icon::Chevron, 10.).text_color(t.text_tertiary));
    if disabled {
        return Some(trigger.opacity(0.45).into_any_element());
    }
    let rows: Vec<Row> = movable
        .iter()
        .map(|(profile, status)| Row {
            profile: profile.clone(),
            name: name(profile, Some(status)),
            picture: bench.accounts.avatar(profile),
            detail: status.email.clone().or_else(|| status.organization.clone()),
            used: accounts::used(bench, profile),
        })
        .collect();
    let on = current.id.clone();
    Some(
        trigger
            .dropdown_menu_with_anchor(
                Anchor::BottomRight,
                ui::menu(move |mut popup, _, _| {
                    popup = popup.min_w(px(280.));
                    for Row {
                        profile,
                        name,
                        picture,
                        detail,
                        used,
                    } in &rows
                    {
                        let here = profile.id == on;
                        let item = accounts::popup_item(
                            profile,
                            name,
                            picture.clone(),
                            detail.clone(),
                            *used,
                            here,
                        );
                        if here {
                            popup = popup.item(item);
                            continue;
                        }
                        let (pick, chosen) = (pick.clone(), profile.clone());
                        popup = popup.item(item.on_click(move |_, _, cx| pick(&chosen, cx)));
                    }
                    if let Some(why) = &none_because {
                        popup = popup.separator().label(why.clone());
                    }
                    popup
                }),
            )
            .into_any_element(),
    )
}

/// An account the menu offers to move to.
struct Row {
    profile: Profile,
    name: String,
    picture: Arc<Image>,
    detail: Option<String>,
    used: Option<f64>,
}
