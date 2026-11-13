//! The account vault on the Connectors page: its recovery key once, or a way back in with it.
//! Every host on the account uses the sign-ins it holds, unless one is kept on this host alone,
//! out of the vault, and the host can make that the rule for new ones.

use gpui_kit::component::input::InputState;
use gpui_kit::component::switch::Switch;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClipboardItem, Context, Div, Entity, IntoElement,
    ParentElement, SharedString, Styled, Window,
};
use sikemux_calls::call;
use sikemux_calls::params::NoParams;
use sikemux_calls::vault::KeepNewParams;
use sikemux_calls::vault::KeepParams;
use sikemux_calls::vault::RecoverParams;
use sikemux_calls::vault::ShownSignIn;
use sikemux_calls::vault::ShownStatus;
use sikemux_calls::vault::ShownVault;

use crate::core::Target;
use crate::settings::rows;
use crate::theme::{self, Tokens, SPACE_2, SPACE_3, SPACE_4, TEXT_XS};
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

/// What the page keeps of the vault while it shows.
#[derive(Default)]
pub struct Fields {
    shown: Option<ShownVault>,
    /// The connector whose sign-ins the person is about to keep on this host, while they decide.
    confirming: Option<String>,
    recovery: Option<Entity<InputState>>,
    refused: Option<SharedString>,
    busy: bool,
}

fn change(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<crate::settings::Settings>),
) {
    super::connectors::change_vault(bench, cx, change);
}

/// Reads the vault.
pub(super) fn read(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::Vault>(NoParams {});
    cx.spawn_in(window, async move |bench, cx| {
        let Ok(shown) = asked.await else { return };
        let _ = bench.update_in(cx, |bench, window, cx| {
            let recovery = (shown.status == ShownStatus::Waiting).then(|| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder(t!("Recovery key"))
                })
            });
            change(bench, cx, |fields, _| {
                fields.shown = Some(shown);
                match recovery {
                    Some(made) if fields.recovery.is_none() => fields.recovery = Some(made),
                    Some(_) => {}
                    None => fields.recovery = None,
                }
            });
        });
    })
    .detach();
}

/// Tells the person how a change went.
fn report(
    asked: impl std::future::Future<Output = Result<(), String>> + 'static,
    done: &'static str,
    cx: &mut Context<Workbench>,
) {
    cx.spawn(async move |bench, cx| {
        let note = match asked.await {
            Ok(()) => toast::success(done),
            Err(error) => toast::error(error),
        };
        cx.update(|cx| toast::show(&bench, note, cx));
    })
    .detach();
}

fn keep(bench: &Workbench, plugin: &str, kept: bool, cx: &mut Context<Workbench>) {
    let mut read = None;
    change(bench, cx, |fields, _| {
        fields.confirming = None;
        read = fields.shown.clone();
    });
    let Some(shown) = read else {
        return;
    };
    let held = if kept { &shown.sign_ins } else { &shown.kept };
    for sign_in in held.iter().filter(|sign_in| sign_in.plugin == plugin) {
        let asked = Target::here(&bench.asks).request_typed::<call::VaultKeep>(KeepParams {
            plugin: sign_in.plugin.clone(),
            identity: sign_in.identity.clone(),
            kept,
        });
        let done = if kept {
            t!("Kept on this host, out of the vault")
        } else {
            t!("Moved into the vault")
        };
        report(asked, done, cx);
    }
}

fn keep_new(bench: &Workbench, on: bool, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::VaultKeepNew>(KeepNewParams { on });
    let done = if on {
        t!("New sign-ins stay on this host")
    } else {
        t!("New sign-ins go into the vault")
    };
    report(asked, done, cx);
}

fn confirm(bench: &Workbench, plugin: Option<String>, cx: &mut App) {
    change(bench, cx, |fields, _| fields.confirming = plugin);
}

fn saved_recovery(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::VaultRecoverySaved>(NoParams {});
    report(asked, t!("Recovery key saved"), cx);
}

fn recover(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let mut typed = None;
    change(bench, cx, |fields, cx| {
        if fields.recovery.is_none() {
            return;
        }
        typed = fields
            .recovery
            .as_ref()
            .map(|input| input.read(cx).value().to_string());
        fields.busy = true;
        fields.refused = None;
    });
    let Some(key) = typed.filter(|key| !key.trim().is_empty()) else {
        change(bench, cx, |fields, _| fields.busy = false);
        return;
    };
    let asked =
        Target::here(&bench.asks).request_typed::<call::VaultRecover>(RecoverParams { key });
    cx.spawn_in(window, async move |bench, cx| {
        let answer = asked.await;
        let _ = bench.update(cx, |bench, cx| {
            change(bench, cx, |fields, _| {
                fields.busy = false;
                fields.refused = answer.err().map(SharedString::from);
            });
        });
    })
    .detach();
}

fn key_box(text: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(SPACE_3)
        .py(SPACE_2)
        .rounded(theme::radius_2())
        .border_1()
        .border_color(t.border)
        .bg(t.surface_sunken)
        .font_family(theme::MONO)
        .text_size(theme::ui_text(TEXT_XS))
        .text_color(t.text_primary)
        .child(text.to_owned())
        .into_any_element()
}

/// The vault's own card: its recovery key once, or the way back in while this host waits.
pub(super) fn section(
    fields: &Fields,
    lit: bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Option<AnyElement> {
    let shown = fields.shown.as_ref()?;
    let t = Tokens::get(cx);
    let meta: Option<SharedString> = match shown.holders {
        0 => None,
        1 => Some(t!("Sealed · 1 host holds the key").into()),
        many => Some(t!("Sealed · {many} hosts hold the key", many = many).into()),
    };
    let quiet = |text: SharedString| {
        div()
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(t.text_secondary)
            .child(text)
    };
    let body = match (&shown.status, &shown.recovery) {
        (ShownStatus::Off, _) => return None,
        (ShownStatus::Open, Some(key)) => {
            let copied = key.clone();
            let saving = this.clone();
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .px(SPACE_4)
                .py(SPACE_3)
                .child(quiet(t!("Save this recovery key somewhere only you can reach. It is shown once, and it is the only way back in if every host that holds the vault is lost.").into()))
                .child(key_box(key, cx))
                .child(
                    div()
                        .flex()
                        .gap(SPACE_2)
                        .child(
                            ui::Button::new("vault-copy-recovery", t!("Copy"), ui::Variant::Outline)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                                }),
                        )
                        .child(
                            ui::Button::new("vault-saved-recovery", t!("I saved it"), ui::Variant::Primary)
                                .on_click(move |_, _, cx| {
                                    saving.update(cx, |bench, cx| saved_recovery(bench, cx))
                                }),
                        ),
                )
                .into_any_element()
        }
        (ShownStatus::Open, None) => {
            let switching = this.clone();
            div()
                .child(rows::note(
                    t!("Sign-ins on this host move into the account's vault, sealed so the accounts server cannot read them, and every host on the account can use them."),
                    cx,
                ))
                .child(rows::row(
                    t!("Keep new sign-ins on this host"),
                    Some(t!("For a host you also use for work. Each new sign-in stays out of the vault until you move it in.")),
                    Switch::new("vault-keep-new")
                        .checked(shown.keep_new)
                        .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
                            let on = *on;
                            switching.update(cx, |bench, cx| keep_new(bench, on, cx))
                        }),
                    cx,
                ))
                .into_any_element()
        }
        (ShownStatus::Waiting, _) => {
            let restoring = this.clone();
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .px(SPACE_4)
                .py(SPACE_3)
                .child(quiet(t!("This host waits for one that holds the vault's key to let it in. If none is left, type the recovery key you saved when the vault was made.").into()))
                .children(fields.recovery.as_ref().map(|input| {
                    div()
                        .flex()
                        .gap(SPACE_2)
                        .child(div().flex_1().child(ui::Field::new(input)))
                        .child(
                            ui::Button::new("vault-recover", t!("Open the vault"), ui::Variant::Primary)
                                .disabled(fields.busy)
                                .on_click(move |_, window, cx| {
                                    restoring.update(cx, |bench, cx| recover(bench, window, cx))
                                }),
                        )
                }))
                .children(fields.refused.clone().map(|refused| {
                    div()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.danger)
                        .child(refused)
                }))
                .into_any_element()
        }
        (ShownStatus::Failed, _) => rows::note(
            shown
                .failed
                .clone()
                .unwrap_or_else(|| t!("The account's vault could not be reached.").into()),
            cx,
        ),
    };
    Some(rows::section(
        n!("Account vault"),
        meta,
        None,
        lit,
        body,
        cx,
    ))
}

fn open_vault(fields: &Fields) -> Option<&ShownVault> {
    fields
        .shown
        .as_ref()
        .filter(|shown| shown.status == ShownStatus::Open)
}

fn of_plugin<'a>(
    held: &'a [ShownSignIn],
    plugin: &'a str,
) -> impl Iterator<Item = &'a ShownSignIn> {
    held.iter().filter(move |sign_in| sign_in.plugin == plugin)
}

/// Keep here only, on a connector signed in from the vault.
pub(super) fn keep_button(
    plugin: &str,
    fields: &Fields,
    this: &Entity<Workbench>,
) -> Option<ui::Button> {
    let shown = open_vault(fields)?;
    of_plugin(&shown.sign_ins, plugin).next()?;
    let others = shown.holders > 1;
    let acting = this.clone();
    let id = plugin.to_owned();
    Some(
        ui::Button::new(
            SharedString::from(format!("vault-keep-{plugin}")),
            t!("Keep here only"),
            ui::Variant::Ghost,
        )
        .on_click(move |_, _, cx| {
            acting.update(cx, |bench, cx| match others {
                true => confirm(bench, Some(id.clone()), cx),
                false => keep(bench, &id, true, cx),
            })
        }),
    )
}

fn under_row(cx: &App) -> Div {
    let t = Tokens::get(cx);
    div()
        .flex()
        .items_center()
        .gap(SPACE_2)
        .pl(px(60.))
        .pr(SPACE_4)
        .py(SPACE_2)
        .border_b_1()
        .border_color(t.line_soft)
        .text_size(theme::ui_text(TEXT_XS))
}

/// Under a connector: the ask before its sign-ins leave the vault, and each one kept on this
/// host with the way to move it back in.
pub(super) fn under(
    plugin: &str,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(shown) = open_vault(fields) else {
        return Vec::new();
    };
    let t = Tokens::get(cx);
    let asking = (fields.confirming.as_deref() == Some(plugin)).then(|| {
        let acting = this.clone();
        let cancelling = this.clone();
        let id = plugin.to_owned();
        under_row(cx)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(t.warn)
                    .child(t!("Every other host on the account loses this sign-in. It leaves the vault and stays on this host alone.")),
            )
            .child(
                ui::Button::new(
                    SharedString::from(format!("vault-keep-cancel-{plugin}")),
                    t!("Cancel"),
                    ui::Variant::Ghost,
                )
                .on_click(move |_, _, cx| cancelling.update(cx, |bench, cx| confirm(bench, None, cx))),
            )
            .child(
                ui::Button::new(
                    SharedString::from(format!("vault-keep-sure-{plugin}")),
                    t!("Keep here only"),
                    ui::Variant::Primary,
                )
                .on_click(move |_, _, cx| acting.update(cx, |bench, cx| keep(bench, &id, true, cx))),
            )
            .into_any_element()
    });
    let kept = of_plugin(&shown.kept, plugin).take(1).map(|_| {
        let moving = this.clone();
        let id = plugin.to_owned();
        under_row(cx)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(t.text_secondary)
                    .child(t!("On this host only, out of the vault")),
            )
            .child(
                ui::Button::new(
                    SharedString::from(format!("vault-share-{plugin}")),
                    t!("Move into vault"),
                    ui::Variant::Outline,
                )
                .on_click(move |_, _, cx| {
                    moving.update(cx, |bench, cx| keep(bench, &id, false, cx))
                }),
            )
            .into_any_element()
    });
    asking.into_iter().chain(kept).collect()
}
