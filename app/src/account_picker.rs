//! Which of the person's accounts with its agent a chat runs on. One that has started moves
//! there, even in the middle of a turn, to those signed in that keep their chats where this
//! one does; one that has not simply starts there, on any signed in.

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::Disableable;
use gpui_kit::{
    div, px, Anchor, AnyElement, Context, Entity, IntoElement, ParentElement, SharedString, Styled,
};
use sikemux_core::agent_accounts::{chat_account, Profile};

use crate::accounts;
use crate::core::Ask;
use crate::icons::Icon;
use crate::theme::{Tokens, SPACE_1H, SPACE_2, TEXT_2XS};
use crate::ui;
use crate::workbench::Workbench;

/// Carries the chat `agent` on to `profile`; a turn at work stops and carries on there.
pub(crate) fn move_chat(bench: &Workbench, agent: &str, profile: &Profile) {
    let account = chat_account(
        &profile.provider,
        &profile.id,
        &profile.name,
        profile.config_path.as_deref(),
    );
    let _ = bench.asks.send(Ask::MoveChat {
        agent: agent.to_owned(),
        account,
    });
}

/// A chat: its agent's id, which agent it is, the account it runs on, and whether it has
/// started.
pub struct Chat<'a> {
    pub agent: &'a str,
    pub provider: &'a str,
    pub on: &'a str,
    pub started: bool,
}

/// The picker for the chat, or none while no other account can take it over.
pub fn picker(
    bench: &Workbench,
    chat: Chat,
    running: bool,
    disabled: bool,
    this: &Entity<Workbench>,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let Chat {
        agent,
        provider,
        on,
        started,
    } = chat;
    let t = Tokens::get(cx).clone();
    let current = bench.accounts.profile(on)?.clone();
    let signed_in = |profile: &Profile| {
        accounts::status(bench, profile)
            .and_then(|status| status.as_ref().ok())
            .filter(|status| status.signed_in)
            .cloned()
    };
    let sessions = signed_in(&current).and_then(|status| status.sessions);
    let movable: Vec<(Profile, accounts::Status)> = bench
        .accounts
        .of(provider)
        .into_iter()
        .filter_map(|profile| Some((profile.clone(), signed_in(profile)?)))
        .filter(|(_, status)| !started || sessions.is_some() && status.sessions == sessions)
        .collect();
    if !movable.iter().any(|(profile, _)| profile.id != current.id) {
        return None;
    }
    let name = |profile: &Profile, status: Option<&accounts::Status>| {
        accounts::name(bench, profile, status)
    };
    let label = name(&current, signed_in(&current).as_ref());
    let about = match running {
        true => "Moving stops this turn and carries it on in the other account".to_owned(),
        false => label.clone(),
    };
    let trigger = KitButton::new(SharedString::from(format!("account-picker-{agent}")))
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
        .child(ui::mark(provider, 13.))
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(label),
        )
        .child(ui::icon(Icon::Chevron, 10.).text_color(t.text_tertiary));
    if disabled {
        return Some(trigger.opacity(0.45).into_any_element());
    }
    let rows: Vec<(Profile, String, Option<String>, Option<f64>)> = movable
        .iter()
        .map(|(profile, status)| {
            let detail = status.email.clone().or_else(|| status.organization.clone());
            (
                profile.clone(),
                name(profile, Some(status)),
                detail,
                accounts::used(bench, profile),
            )
        })
        .collect();
    let (this, agent) = (this.clone(), agent.to_owned());
    let on = current.id.clone();
    Some(
        trigger
            .dropdown_menu_with_anchor(
                Anchor::BottomRight,
                ui::menu(move |mut popup, _, _| {
                    popup = popup.min_w(px(280.));
                    for (profile, name, detail, used) in &rows {
                        let here = profile.id == on;
                        let item = accounts::popup_item(profile, name, detail.clone(), *used, here);
                        if here {
                            popup = popup.item(item);
                            continue;
                        }
                        let (this, agent, chosen) = (this.clone(), agent.clone(), profile.clone());
                        popup = popup.item(item.on_click(move |_, _, cx| {
                            this.update(cx, |bench, _| match started {
                                true => move_chat(bench, &agent, &chosen),
                                false => bench.start_as(
                                    &agent,
                                    serde_json::json!({ "profileId": chosen.id }),
                                    &chosen.name,
                                ),
                            });
                        }));
                    }
                    popup
                }),
            )
            .into_any_element(),
    )
}
