//! The person's accounts with their agents, as the core keeps them: who each
//! is signed in as, and signing one in through its agent's own login, which
//! opens the browser on this host.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui_kit::component::input::InputState;
use gpui_kit::component::menu::PopupMenuItem;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    div, px, AppContext, Context, Entity, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};
use sikemux_core::agent_accounts::status::AccountStatus;
use sikemux_core::agent_accounts::{Book, Profile};
use sikemux_core::agent_catalog::AgentKind;
use sikemux_core::server::agent_accounts::{
    Account as AgentAccount, AccountUsage, Adding, Choosing, Forget, Removing, Saving, Switching,
};
use sikemux_core::server::agent_sign_in::{Account as SignInAccount, Code};
use sikemux_core::server::call::{self, Call};
use sikemux_core::server::calls::NoParams;

use crate::core::{typed_call, Target};
use crate::theme;
use crate::theme::{SPACE_2, TEXT_3XS_H};
use crate::workbench::Workbench;
use sikemux_core::agent_accounts::BUILT_IN;

/// An account's sign-in while it runs or after it failed.
pub enum SignIn {
    /// The browser is out; `url` is the page it opened, once the core has it,
    /// and `code` the field for the code some pages show.
    Waiting {
        url: Option<String>,
        code: Option<Entity<InputState>>,
    },
    Failed(String),
}

pub struct Dialog {
    pub provider: String,
    pub name: Entity<InputState>,
    /// The account signing in, once it has one.
    pub account: Option<String>,
    /// The account this dialog made, taken away again if it closes before signing in.
    pub made: Option<String>,
    pub busy: bool,
}

#[derive(Default)]
pub struct Accounts {
    pub book: Option<Book>,
    asked_book: bool,
    status: HashMap<(String, Option<String>), Result<AccountStatus, String>>,
    asking: HashSet<(String, Option<String>)>,
    pub signing: HashMap<String, SignIn>,
    pub dialog: Option<Dialog>,
    /// The chats that failed for want of an account's sign-in, by that account, to move onto
    /// it once it signs in.
    pub waiting_chats: HashMap<String, Vec<String>>,
    /// Whether the Limits panel is open onto every account, while the pointer rests on it.
    pub limits_open: bool,
    /// When coming back to the app last had the core check every agent's sign-in.
    checked_on_return: Option<Instant>,
}

/// Coming back to the app often comes in bursts, and each check starts the agents' CLIs, so it
/// happens at most this often, as on the Mac.
const CHECK_ON_RETURN_AT_MOST_EVERY: Duration = Duration::from_secs(30);

/// An account's colour, which the person may have set as `#rrggbb`.
pub fn accent(profile: &Profile) -> Hsla {
    profile
        .accent
        .strip_prefix('#')
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .map(|value| gpui_kit::rgb(value).into())
        .unwrap_or_else(|| theme::brand(&profile.provider))
}

/// The account's first letter on its colour.
pub fn avatar(profile: &Profile, name: &str, size: f32) -> impl IntoElement {
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .bg(accent(profile))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.5))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui_kit::white())
        .child(
            name.trim()
                .chars()
                .next()
                .map(|letter| letter.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into()),
        )
}

/// An account as a row of a menu, as the Mac draws one: its letter, its name with a tick on the
/// one in use, what it is signed in as under that, and how much of its plan it has used.
pub fn popup_item(
    profile: &Profile,
    name: &str,
    detail: Option<String>,
    used: Option<f64>,
    current: bool,
) -> PopupMenuItem {
    let (profile, name) = (profile.clone(), name.to_owned());
    PopupMenuItem::element(move |_, cx| {
        let t = crate::theme::Tokens::get(cx).clone();
        div()
            .id(SharedString::from(format!("account-{}", profile.id)))
            .aria_label(name.clone())
            .test_support()
            .w_full()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .child(avatar(&profile, &name, 16.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .child(div().min_w_0().truncate().child(name.clone()))
                            .when(current, |line| {
                                line.child(
                                    crate::ui::icon(crate::icons::Icon::Check, 11.)
                                        .text_color(t.accent),
                                )
                            }),
                    )
                    .children(detail.clone().map(|detail| {
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(theme::ui_text(TEXT_3XS_H))
                            .text_color(t.text_tertiary)
                            .child(detail)
                    })),
            )
            .children(used.map(|used| {
                div()
                    .flex_none()
                    .font_family(theme::MONO)
                    .text_size(theme::ui_text(TEXT_3XS_H))
                    .text_color(t.text_tertiary)
                    .child(format!("{}%", used.round()))
            }))
    })
}

fn key(profile: &Profile) -> (String, Option<String>) {
    (profile.provider.clone(), profile.config_path.clone())
}

fn redraw(bench: &Workbench, cx: &mut Context<Workbench>) {
    bench.parts.redraw_stage(cx);
}

impl Accounts {
    pub fn profile(&self, id: &str) -> Option<&Profile> {
        self.book
            .as_ref()?
            .profiles
            .iter()
            .find(|profile| profile.id == id)
    }

    /// The accounts `provider` has, in the order they were added.
    pub fn of(&self, provider: &str) -> Vec<&Profile> {
        self.book
            .iter()
            .flat_map(|book| &book.profiles)
            .filter(|profile| profile.provider == provider)
            .collect()
    }

    /// The account `provider` runs on now, else its first.
    pub fn current(&self, provider: &str) -> Option<&Profile> {
        let chosen = self.book.as_ref()?.selected.get(provider);
        let accounts = self.of(provider);
        chosen
            .and_then(|id| accounts.iter().find(|profile| profile.id == *id))
            .or(accounts.first())
            .copied()
    }

    pub fn auto_switch(&self, provider: &str) -> bool {
        self.book
            .as_ref()
            .and_then(|book| book.auto_switch.get(provider).copied())
            .unwrap_or(false)
    }
}

/// Reads the accounts the core keeps, the first time something shows them.
pub fn read(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if !std::mem::replace(&mut bench.accounts.asked_book, true) {
        fetch(bench, cx);
    }
}

/// Reads the accounts again when the core says they changed, once something showed them.
pub fn book_moved(bench: &Workbench, cx: &mut Context<Workbench>) {
    if bench.accounts.asked_book {
        fetch(bench, cx);
    }
}

fn fetch(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let book = Target::here(&asks)
            .request_typed::<call::AgentAccounts>(NoParams {})
            .await;
        let _ = bench.update(cx, |bench, cx| {
            adopt(bench, book.unwrap_or_default(), cx);
        });
    })
    .detach();
}

/// Keeps the accounts as the core answered them after a change.
fn adopt(bench: &mut Workbench, book: Book, cx: &mut Context<Workbench>) {
    bench.accounts.book = Some(book);
    ask_statuses(bench, cx);
    redraw(bench, cx);
}

/// Changes the accounts in the core with the call `C`, then keeps what it answers.
fn change<C: Call<Answer = Book> + 'static>(
    bench: &Workbench,
    params: C::Params,
    cx: &mut Context<Workbench>,
) where
    C::Params: serde::Serialize,
{
    let asked = Target::here(&bench.asks).request_typed::<C>(params);
    cx.spawn(async move |bench, cx| {
        let Ok(book) = asked.await else {
            return;
        };
        let _ = bench.update(cx, |bench, cx| adopt(bench, book, cx));
    })
    .detach();
}

pub fn save(bench: &Workbench, profile: &Profile, cx: &mut Context<Workbench>) {
    let profile = profile.clone();
    change::<call::SaveAgentAccount>(bench, Saving { profile }, cx);
}

pub fn remove(bench: &Workbench, id: &str, cx: &mut Context<Workbench>) {
    let id = id.to_owned();
    change::<call::RemoveAgentAccount>(bench, Removing { id }, cx);
}

/// Makes the account the one its agent runs on; the core moves open chats there.
pub fn select(bench: &Workbench, profile: &Profile, cx: &mut Context<Workbench>) {
    let params = Choosing {
        provider: profile.provider.clone(),
        id: profile.id.clone(),
    };
    change::<call::SelectAgentAccount>(bench, params, cx);
}

pub fn set_auto_switch(bench: &Workbench, provider: &str, on: bool, cx: &mut Context<Workbench>) {
    let params = Switching {
        provider: provider.to_owned(),
        on,
    };
    change::<call::SetAccountAutoSwitch>(bench, params, cx);
}

/// What the account is called: who it is signed in as while it still has its agent's own
/// name, else the name the person gave it.
pub fn name(bench: &Workbench, profile: &Profile, status: Option<&AccountStatus>) -> String {
    let unnamed = profile.id.starts_with(BUILT_IN)
        && bench
            .reads
            .catalog
            .kind(&profile.provider)
            .is_some_and(|kind| kind.label == profile.name);
    status
        .and_then(|status| status.name.clone())
        .filter(|_| unnamed)
        .unwrap_or_else(|| profile.name.clone())
}

/// The launcher that starts agents on the account: its own, or the agent's for a built-in
/// account the core offers as the agent itself.
fn launcher_of(bench: &Workbench, profile: &Profile) -> String {
    let own = format!("{}:{}", profile.provider, profile.id);
    match bench.usage_of(&own) {
        None if profile.id.starts_with(BUILT_IN) => profile.provider.clone(),
        _ => own,
    }
}

/// The account's plan usage as the core last read it, or why it has none.
pub fn usage<'a>(bench: &'a Workbench, profile: &Profile) -> Option<&'a AccountUsage> {
    bench.usage_of(&launcher_of(bench, profile))
}

/// The most of any window of its plan the account has used, as the core last read it.
pub fn used(bench: &Workbench, profile: &Profile) -> Option<f64> {
    bench.usage_peak(&launcher_of(bench, profile))
}

/// Opens Settings on its agents with the add-account dialog for `provider`, signing in
/// the account `id` there when there is one.
pub fn open_sign_in(
    bench: &mut Workbench,
    provider: &str,
    id: Option<&str>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    bench.open_settings(crate::settings::Page::Accounts, window, cx);
    open_dialog(bench, provider, window, cx);
    if let Some(id) = id {
        sign_in_again(bench, id, cx);
    }
}

/// Who the account is signed in as, once its agent has said.
pub fn status<'a>(
    bench: &'a Workbench,
    profile: &Profile,
) -> Option<&'a Result<AccountStatus, String>> {
    bench.accounts.status.get(&key(profile))
}

/// Asks each account's agent who it is signed in as, where nobody has yet.
fn ask_statuses(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let unasked: Vec<(String, Option<String>)> = bench
        .accounts
        .book
        .iter()
        .flat_map(|book| &book.profiles)
        .map(key)
        .filter(|key| !bench.accounts.status.contains_key(key))
        .collect();
    ask(bench, unasked, cx);
}

/// Asks each of the agent's accounts who it is signed in as again, as refreshing its plan limits
/// does; what each said before shows until it answers.
pub fn ask_again(bench: &mut Workbench, provider: &str, cx: &mut Context<Workbench>) {
    let accounts = bench.accounts.of(provider).into_iter().map(key).collect();
    ask(bench, accounts, cx);
}

fn ask(
    bench: &mut Workbench,
    accounts: Vec<(String, Option<String>)>,
    cx: &mut Context<Workbench>,
) {
    let unasked: Vec<_> = accounts
        .into_iter()
        .filter(|key| !bench.accounts.asking.contains(key))
        .collect();
    for asked in unasked {
        bench.accounts.asking.insert(asked.clone());
        let asks = bench.asks.clone();
        let params = AgentAccount {
            provider: asked.0.clone(),
            config_path: asked.1.clone(),
        };
        cx.spawn(async move |bench, cx| {
            let status = Target::here(&asks)
                .request_typed::<call::AgentAccountStatus>(params)
                .await;
            let _ = bench.update(cx, |bench, cx| {
                bench.accounts.asking.remove(&asked);
                bench.accounts.status.insert(asked, status);
                redraw(bench, cx);
            });
        })
        .detach();
    }
}

/// Asks every account who it is signed in as again, as the core says an agent's sign-in changed;
/// what each said before shows until it answers.
pub fn statuses_moved(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    if !bench.accounts.asked_book {
        return;
    }
    let accounts = bench
        .accounts
        .book
        .iter()
        .flat_map(|book| &book.profiles)
        .map(key)
        .collect();
    ask(bench, accounts, cx);
}

/// Has the core check every agent's sign-in again as the person comes back to the app, since
/// they may have signed in or out in a terminal, at most every half minute.
pub fn check_on_return(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let now = Instant::now();
    let recent = bench
        .accounts
        .checked_on_return
        .is_some_and(|at| now.duration_since(at) < CHECK_ON_RETURN_AT_MOST_EVERY);
    if recent || !bench.accounts.asked_book {
        return;
    }
    bench.accounts.checked_on_return = Some(now);
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let forgot =
            Target::here(&asks).request_typed::<call::ForgetAgentStatus>(Forget::default());
        let _ = forgot.await;
        let _ = bench.update(cx, statuses_moved);
    })
    .detach();
}

/// Marks the account to be asked about again, since its sign-in changed.
fn forget(bench: &mut Workbench, profile: &Profile, cx: &mut Context<Workbench>) {
    let key = key(profile);
    bench.accounts.status.remove(&key);
    let asks = bench.asks.clone();
    let params = Forget {
        provider: Some(key.0),
        config_path: key.1,
    };
    cx.spawn(async move |bench, cx| {
        let forgot = Target::here(&asks).request_typed::<call::ForgetAgentStatus>(params);
        let _ = forgot.await;
        let _ = bench.update(cx, |bench, cx| {
            ask_statuses(bench, cx);
            redraw(bench, cx);
        });
    })
    .detach();
}

/// The account as its sign-in calls name it; none for an agent the core has no sign-in for.
fn signing_in(profile: &Profile) -> Result<SignInAccount, String> {
    let agent = AgentKind::from_name(&profile.provider)
        .ok_or_else(|| format!("{} is not an agent Sikemux knows", profile.provider))?;
    Ok(SignInAccount {
        agent,
        executable_path: profile.executable_path.clone(),
        config_path: profile.config_path.clone(),
    })
}

/// Runs the account's own sign-in. The page it opens is kept for when no browser came up.
pub fn sign_in(bench: &mut Workbench, id: &str, cx: &mut Context<Workbench>) {
    let Some(profile) = bench.accounts.profile(id).cloned() else {
        return;
    };
    if matches!(bench.accounts.signing.get(id), Some(SignIn::Waiting { .. })) {
        return;
    }
    bench.accounts.signing.insert(
        id.to_owned(),
        SignIn::Waiting {
            url: None,
            code: None,
        },
    );
    redraw(bench, cx);
    let target = Target::here(&bench.asks);
    let params = signing_in(&profile);
    cx.spawn(async move |bench, cx| {
        let answer = match params {
            Ok(params) => target.request_typed::<call::AgentSignIn>(params).await,
            Err(error) => Err(error),
        };
        let _ = bench.update(cx, |bench, cx| {
            match answer {
                Err(message) if message != "Sign-in stopped" => {
                    bench
                        .accounts
                        .signing
                        .insert(profile.id.clone(), SignIn::Failed(message));
                }
                Err(_) => {
                    bench.accounts.signing.remove(&profile.id);
                }
                Ok(_) => {
                    bench.accounts.signing.remove(&profile.id);
                    let chats = bench.accounts.waiting_chats.remove(&profile.id);
                    for chat in chats.into_iter().flatten() {
                        crate::account_picker::move_chat(bench, &chat, &profile);
                    }
                }
            }
            forget(bench, &profile, cx);
        });
    })
    .detach();
}

/// Hands each waiting sign-in the page its agent opened, when the core says one did.
pub fn pages_moved(bench: &Workbench, cx: &mut Context<Workbench>) {
    if !bench
        .accounts
        .signing
        .values()
        .any(|signing| matches!(signing, SignIn::Waiting { url: None, .. }))
    {
        return;
    }
    let asks = bench.asks.clone();
    cx.spawn(async move |bench, cx| {
        let asked = Target::here(&asks).request_typed::<call::AgentSignIns>(NoParams {});
        let Ok(pages) = asked.await else {
            return;
        };
        let _ = bench.update(cx, |bench, cx| {
            let Workbench { accounts, .. } = bench;
            for page in pages {
                let opened_for =
                    accounts
                        .book
                        .iter()
                        .flat_map(|book| &book.profiles)
                        .find(|profile| {
                            page.agent == profile.provider
                                && profile.config_path == page.config_path
                        });
                let Some(id) = opened_for.map(|profile| profile.id.clone()) else {
                    continue;
                };
                if let Some(SignIn::Waiting { url, .. }) = accounts.signing.get_mut(&id) {
                    *url = Some(page.url);
                }
            }
            redraw(bench, cx);
        });
    })
    .detach();
}

pub fn cancel_sign_in(bench: &Workbench, id: &str) {
    let Some(profile) = bench.accounts.profile(id) else {
        return;
    };
    if let Ok(account) = signing_in(profile) {
        let _ = bench
            .asks
            .send(typed_call::<call::CancelAgentSignIn>(account));
    }
}

pub fn dismiss_failure(bench: &mut Workbench, id: &str) {
    if matches!(bench.accounts.signing.get(id), Some(SignIn::Failed(_))) {
        bench.accounts.signing.remove(id);
    }
}

/// Opens the field for the code the sign-in page showed.
pub fn ask_code(bench: &mut Workbench, id: &str, window: &mut Window, cx: &mut Context<Workbench>) {
    let field = cx.new(|cx| InputState::new(window, cx).placeholder("Code from the page"));
    field.update(cx, |field, cx| field.focus(window, cx));
    if let Some(SignIn::Waiting { code, .. }) = bench.accounts.signing.get_mut(id) {
        *code = Some(field);
    }
    redraw(bench, cx);
}

/// Hands the running sign-in the code typed into its field.
pub fn submit_code(bench: &mut Workbench, id: &str, cx: &mut Context<Workbench>) {
    let Some(profile) = bench.accounts.profile(id).cloned() else {
        return;
    };
    let Some(SignIn::Waiting {
        code: Some(field), ..
    }) = bench.accounts.signing.get(id)
    else {
        return;
    };
    let code = field.read(cx).value().trim().to_owned();
    if code.is_empty() {
        return;
    }
    let Ok(account) = signing_in(&profile) else {
        return;
    };
    let asked =
        Target::here(&bench.asks).request_typed::<call::AgentSignInCode>(Code { account, code });
    cx.spawn(async move |bench, cx| {
        if asked.await.is_ok() {
            let _ = bench.update(cx, |bench, cx| {
                if let Some(SignIn::Waiting { code, .. }) =
                    bench.accounts.signing.get_mut(&profile.id)
                {
                    *code = None;
                }
                redraw(bench, cx);
            });
        }
    })
    .detach();
}

pub fn sign_out(bench: &Workbench, id: &str, cx: &mut Context<Workbench>) {
    let Some(profile) = bench.accounts.profile(id).cloned() else {
        return;
    };
    let Ok(account) = signing_in(&profile) else {
        return;
    };
    let asked = Target::here(&bench.asks).request_typed::<call::AgentSignOut>(account);
    cx.spawn(async move |bench, cx| {
        let _ = asked.await;
        let _ = bench.update(cx, |bench, cx| forget(bench, &profile, cx));
    })
    .detach();
}

/// Opens the dialog that adds an account to `provider`.
pub fn open_dialog(
    bench: &mut Workbench,
    provider: &str,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let name = cx.new(|cx| InputState::new(window, cx).placeholder("Work"));
    name.update(cx, |name, cx| name.focus(window, cx));
    bench.accounts.dialog = Some(Dialog {
        provider: provider.to_owned(),
        name,
        account: None,
        made: None,
        busy: false,
    });
    redraw(bench, cx);
}

/// Signs an account already there in again, from the dialog.
pub fn sign_in_again(bench: &mut Workbench, id: &str, cx: &mut Context<Workbench>) {
    if let Some(dialog) = bench.accounts.dialog.as_mut() {
        dialog.account = Some(id.to_owned());
    }
    dismiss_failure(bench, id);
    sign_in(bench, id, cx);
}

/// Makes the account named in the dialog: a folder of its own sharing the default one's
/// chats and settings, then signs it in.
pub fn add(bench: &mut Workbench, brand: String, cx: &mut Context<Workbench>) {
    let Some(dialog) = bench.accounts.dialog.as_mut() else {
        return;
    };
    let name = dialog.name.read(cx).value().trim().to_owned();
    if name.is_empty() || dialog.busy {
        return;
    }
    dialog.busy = true;
    let provider = dialog.provider.clone();
    let asks = bench.asks.clone();
    redraw(bench, cx);
    cx.spawn(async move |bench, cx| {
        let made = async {
            let here = Target::here(&asks);
            let folder = here
                .request_typed::<call::AddAgentAccount>(Adding {
                    provider: provider.clone(),
                    name: name.clone(),
                })
                .await?;
            let now = chrono::Utc::now().timestamp_millis();
            let profile = Profile {
                id: format!("profile-{now:x}"),
                name,
                provider,
                accent: brand,
                executable_path: None,
                config_path: Some(folder),
                environment_keys: Vec::new(),
            };
            let id = profile.id.clone();
            let book = here
                .request_typed::<call::SaveAgentAccount>(Saving { profile })
                .await?;
            Ok::<_, String>((id, book))
        }
        .await;
        let _ = bench.update(cx, |bench, cx| {
            if let Some(dialog) = bench.accounts.dialog.as_mut() {
                dialog.busy = false;
            }
            let Ok((id, book)) = made else {
                return redraw(bench, cx);
            };
            adopt(bench, book, cx);
            if let Some(dialog) = bench.accounts.dialog.as_mut() {
                dialog.made = Some(id.clone());
                dialog.account = Some(id.clone());
            }
            sign_in(bench, &id, cx);
        });
    })
    .detach();
}

/// Closes the dialog, stopping a sign-in still waiting and taking away an account it
/// made that never signed in.
pub fn close_dialog(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(dialog) = bench.accounts.dialog.take() else {
        return;
    };
    if let Some(id) = &dialog.account {
        let waiting = matches!(bench.accounts.signing.get(id), Some(SignIn::Waiting { .. }));
        if waiting {
            cancel_sign_in(bench, id);
        }
        let unfinished = bench.accounts.signing.contains_key(id);
        if dialog.made.as_deref() == Some(id.as_str()) && unfinished {
            remove(bench, id, cx);
        }
    }
    redraw(bench, cx);
}
