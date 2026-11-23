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
use sikemux_agents::agent_accounts::status::AccountStatus;
use sikemux_agents::agent_accounts::Profile;
use sikemux_calls::agent_accounts::ChatAccounts;
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
    let takers = accounts::takers(bench, key, on, cx).cloned();
    let smart = takers.as_ref().and_then(|takers| takers.smart.clone());
    let profiles = bench.accounts.of(provider);
    let (movable, none_because) = listed(
        &profiles,
        &current,
        started.is_some(),
        takers.as_ref(),
        |profile| {
            accounts::status(bench, profile)
                .and_then(|status| status.as_ref().ok())
                .cloned()
        },
    );
    let name = |profile: &Profile, status: Option<&AccountStatus>| profile.shown_name(status);
    let label = name(
        &current,
        accounts::status(bench, &current).and_then(|status| status.as_ref().ok()),
    );
    let about = match (&none_because, running) {
        (Some(why), _) => why.clone(),
        (None, true) => {
            t!("Moving stops this turn and carries it on in the other account").to_owned()
        }
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
        .child(accounts::face(
            bench.accounts.avatar(&current),
            provider,
            16.,
            cx,
        ))
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
    let rows: Vec<Row> = movable
        .iter()
        .map(|(profile, status)| Row {
            profile: profile.clone(),
            name: name(profile, status.as_ref()),
            picture: bench.accounts.avatar(profile),
            used: accounts::used(bench, profile),
        })
        .collect();
    let on = current.id;
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
                        used,
                    } in &rows
                    {
                        let here = profile.id == on;
                        let item = accounts::popup_item(
                            profile,
                            name,
                            picture.clone(),
                            *used,
                            here,
                            smart.as_deref() == Some(profile.id.as_str()),
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
    used: Option<f64>,
}

fn listed(
    profiles: &[&Profile],
    current: &Profile,
    started: bool,
    takers: Option<&ChatAccounts>,
    status_of: impl Fn(&Profile) -> Option<AccountStatus>,
) -> (Vec<(Profile, Option<AccountStatus>)>, Option<String>) {
    let signed_out = |profile: &Profile| status_of(profile).is_some_and(|status| !status.signed_in);
    let matching = takers.filter(|takers| {
        takers
            .account
            .as_ref()
            .is_none_or(|account| account.id == current.id)
    });
    match started {
        true => match matching {
            Some(takers) => {
                let movable = profiles
                    .iter()
                    .copied()
                    .filter(|profile| {
                        profile.id == current.id
                            || takers.others.iter().any(|other| other.id == profile.id)
                    })
                    .map(|profile| (profile.clone(), status_of(profile)))
                    .collect();
                (movable, takers.none_because.clone())
            }
            None => {
                let movable = profiles
                    .iter()
                    .copied()
                    .filter(|profile| profile.id == current.id || !signed_out(profile))
                    .map(|profile| (profile.clone(), status_of(profile)))
                    .collect();
                (movable, None)
            }
        },
        false => {
            let movable: Vec<_> = profiles
                .iter()
                .copied()
                .filter(|profile| profile.id == current.id || !signed_out(profile))
                .map(|profile| (profile.clone(), status_of(profile)))
                .collect();
            let alone = !movable.iter().any(|(profile, _)| profile.id != current.id);
            (
                movable,
                alone.then(|| t!("No other account is signed in").to_owned()),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sikemux_calls::agent_accounts::NamedAccount;

    fn profile(id: &str, name: &str) -> Profile {
        Profile {
            id: id.into(),
            name: name.into(),
            provider: "claude".into(),
            accent: "#d97757".into(),
            executable_path: None,
            config_path: None,
            environment_keys: Vec::new(),
        }
    }

    fn signed_in_status() -> AccountStatus {
        AccountStatus {
            signed_in: true,
            ..AccountStatus::default()
        }
    }

    #[test]
    fn picker_lists_every_signed_in_account_while_takers_loads_or_is_stale() {
        let kishore = profile("kishore", "Kishore");
        let pers = profile("kishore-pers", "Kishore Pers");
        let all = [&kishore, &pers];

        // 1. While ChatAccounts RPC is still in flight (takers == None), both signed-in accounts must show.
        let (rows, _) = listed(&all, &pers, true, None, |_| Some(signed_in_status()));
        let ids: Vec<&str> = rows.iter().map(|(p, _)| p.id.as_str()).collect();
        assert_eq!(ids, vec!["kishore", "kishore-pers"]);

        // 2. When a stale ChatAccounts answer for the other account (account == kishore,
        //    others == [kishore-pers]) is present while current == kishore-pers, Kishore must not vanish.
        let stale = ChatAccounts {
            account: Some(NamedAccount {
                id: "kishore".into(),
                label: "Kishore".into(),
            }),
            others: vec![NamedAccount {
                id: "kishore-pers".into(),
                label: "Kishore Pers".into(),
            }],
            none_because: None,
            smart: Some("kishore".into()),
        };
        let (rows, _) = listed(
            &all,
            &pers,
            true,
            Some(&stale),
            |_| Some(signed_in_status()),
        );
        let ids: Vec<&str> = rows.iter().map(|(p, _)| p.id.as_str()).collect();
        assert_eq!(ids, vec!["kishore", "kishore-pers"]);

        // 3. When takers confirms Kishore can take the chat, missing/errored status must not drop it.
        let fresh = ChatAccounts {
            account: Some(NamedAccount {
                id: "kishore-pers".into(),
                label: "Kishore Pers".into(),
            }),
            others: vec![NamedAccount {
                id: "kishore".into(),
                label: "Kishore".into(),
            }],
            none_because: None,
            smart: Some("kishore".into()),
        };
        let (rows, _) = listed(&all, &pers, true, Some(&fresh), |p| {
            (p.id == "kishore-pers").then(signed_in_status)
        });
        let ids: Vec<&str> = rows.iter().map(|(p, _)| p.id.as_str()).collect();
        assert_eq!(ids, vec!["kishore", "kishore-pers"]);
    }
}
