//! Connector sync (the account's sign-in vault) on the Connectors page: quiet one-line footer
//! when healthy, prominent recovery banner when a recovery key is needed or waiting to be saved.

use gpui_kit::component::input::InputState;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClipboardItem, Context, Entity, FontWeight, IntoElement,
    ParentElement, SharedString, Styled, Window,
};
use sikemux_calls::call;
use sikemux_calls::params::NoParams;
use sikemux_calls::vault::{
    KeepNewParams, KeepParams, RecoverParams, ShownSignIn, ShownStatus, ShownVault,
};

use crate::core::Target;
use crate::icons::Icon;
use crate::settings::rows;
use crate::theme::{self, Tokens, SPACE_1, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS, TEXT_SM, TEXT_XS};
use crate::toast;
use crate::ui::{self, Tone};
use crate::workbench::Workbench;

#[derive(Default)]
pub struct Fields {
    shown: Option<ShownVault>,
    confirming: Option<String>,
    recovery: Option<Entity<InputState>>,
    refused: Option<SharedString>,
    busy: bool,
    expanded: bool,
    pub(super) stale: bool,
}

fn change(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<crate::settings::Settings>),
) {
    super::connectors::change_vault(bench, cx, change);
}

pub(super) fn read(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    change(bench, cx, |fields, _| fields.stale = false);
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
            t!("Kept on this host only")
        } else {
            t!("Synced to your hosts")
        };
        report(asked, done, cx);
    }
}

pub(super) fn toggle_keep_local(
    bench: &Workbench,
    plugin: &str,
    kept: bool,
    cx: &mut Context<Workbench>,
) {
    let others = super::state(bench, cx)
        .and_then(|pages| pages.connectors.as_ref())
        .and_then(|fields| open_vault(&fields.vault))
        .is_some_and(|shown| shown.holders > 1);
    if kept && others {
        confirm(bench, Some(plugin.to_owned()), cx);
    } else {
        keep(bench, plugin, kept, cx);
    }
}

fn keep_new(bench: &Workbench, on: bool, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::VaultKeepNew>(KeepNewParams { on });
    let done = if on {
        t!("New sign-ins stay on this host")
    } else {
        t!("New sign-ins sync to your hosts")
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

fn show_recovery(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::VaultShowRecovery>(NoParams {});
    cx.spawn(async move |bench, cx| {
        if let Err(error) = asked.await {
            cx.update(|cx| toast::show(&bench, toast::error(error), cx));
        }
    })
    .detach();
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
            let ok = answer.is_ok();
            change(bench, cx, |fields, _| {
                fields.busy = false;
                fields.refused = answer.err().map(SharedString::from);
            });
            if ok {
                bench.notify(
                    toast::success(t!("Recovery key accepted · sign-ins unlocked here")),
                    cx,
                );
            }
        });
    })
    .detach();
}

pub(super) fn key_box(text: &str, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    div()
        .px(SPACE_3)
        .py(SPACE_2)
        .rounded(theme::radius_2())
        .border(crate::theme::border())
        .border_color(t.border)
        .bg(t.surface_sunken)
        .font_family(theme::mono_font())
        .text_size(theme::ui_text(TEXT_XS))
        .text_color(t.text_primary)
        .child(text.to_owned())
        .into_any_element()
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

pub(super) fn is_waiting(fields: &Fields) -> bool {
    fields
        .shown
        .as_ref()
        .is_some_and(|shown| shown.status == ShownStatus::Waiting)
}

pub(super) fn has_sign_in(plugin: &str, fields: &Fields) -> bool {
    let Some(shown) = fields.shown.as_ref() else {
        return false;
    };
    of_plugin(&shown.sign_ins, plugin).next().is_some()
        || of_plugin(&shown.kept, plugin).next().is_some()
}

pub(super) fn is_kept_local(plugin: &str, fields: &Fields) -> bool {
    let Some(shown) = open_vault(fields) else {
        return false;
    };
    of_plugin(&shown.kept, plugin).next().is_some()
        && of_plugin(&shown.sign_ins, plugin).next().is_none()
}

pub(super) fn is_synced(plugin: &str, fields: &Fields) -> bool {
    let Some(shown) = open_vault(fields) else {
        return false;
    };
    of_plugin(&shown.sign_ins, plugin).next().is_some()
}

pub(super) fn is_locked(plugin: &str, fields: &Fields) -> bool {
    is_waiting(fields) && has_sign_in(plugin, fields)
}

/// Some(true) if kept on this host only, Some(false) if synced across hosts, None if unavailable.
pub(super) fn local_toggle_state(plugin: &str, fields: &Fields) -> Option<bool> {
    if is_synced(plugin, fields) {
        Some(false)
    } else if is_kept_local(plugin, fields) {
        Some(true)
    } else {
        None
    }
}

/// Prominent warning or one-time recovery card at the top of Connectors when attention is needed.
pub(super) fn top_banner(
    fields: &Fields,
    lit: bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Option<AnyElement> {
    let shown = fields.shown.as_ref()?;
    let t = Tokens::get(cx);
    let warn_card = |content: AnyElement| {
        div()
            .flex()
            .items_start()
            .gap(SPACE_3)
            .px(SPACE_4)
            .py(SPACE_3)
            .rounded(theme::radius_3())
            .border(crate::theme::border())
            .border_color(if lit { t.accent } else { t.warn.opacity(0.32) })
            .bg(t.warn.opacity(0.06))
            .child(
                div()
                    .pt(px(2.))
                    .flex_none()
                    .child(ui::icon(Icon::Warning, 16.).text_color(t.warn)),
            )
            .child(div().flex_1().min_w_0().child(content))
            .into_any_element()
    };
    let title = |text: SharedString| {
        div()
            .text_size(theme::ui_text(TEXT_SM))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.text_primary)
            .child(text)
    };
    let desc = |text: SharedString| {
        div()
            .text_size(theme::ui_text(TEXT_XS))
            .text_color(t.text_secondary)
            .child(text)
    };
    match (&shown.status, &shown.recovery) {
        (ShownStatus::Waiting, _) => {
            let restoring = this.clone();
            let content = div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(title(t!("Connector sync is waiting for this host").into()))
                .child(desc(
                    t!("This host waits for another host to let it in. Enter your recovery key once to unseal your sign-ins here.").into(),
                ))
                .children(fields.recovery.as_ref().map(|input| {
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .pt(SPACE_1)
                        .child(div().flex_1().min_w_0().child(ui::Field::new(input)))
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
                .into_any_element();
            Some(warn_card(content))
        }
        (ShownStatus::Open, Some(key)) => {
            let copied = key.clone();
            let copying = this.clone();
            let saving = this.clone();
            let content = div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(title(t!("Save your connector sync recovery key").into()))
                .child(desc(
                    t!("Save this recovery key somewhere only you can reach. It is shown once, and is the only way back in if every host that holds it is lost.").into(),
                ))
                .child(key_box(key, cx))
                .child(
                    div()
                        .flex()
                        .gap(SPACE_2)
                        .child(
                            ui::Button::new("vault-copy-recovery", t!("Copy"), ui::Variant::Outline)
                                .icon(Icon::Copy)
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                                    copying.update(cx, |bench, cx| {
                                        bench.notify(toast::success(t!("Copied recovery key")), cx)
                                    });
                                }),
                        )
                        .child(
                            ui::Button::new(
                                "vault-saved-recovery",
                                t!("I saved it"),
                                ui::Variant::Primary,
                            )
                            .on_click(move |_, _, cx| {
                                saving.update(cx, |bench, cx| saved_recovery(bench, cx))
                            }),
                        ),
                )
                .into_any_element();
            Some(warn_card(content))
        }
        (ShownStatus::Open, None) if shown.recovery_in_window => {
            let showing = this.clone();
            let content = div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(title(t!("Save your connector sync recovery key").into()))
                .child(desc(
                    t!("Save the recovery key somewhere only you can reach. Sikemux shows it in a window of its own, out of reach of your other programs.").into(),
                ))
                .child(
                    div().flex().child(
                        ui::Button::new(
                            "vault-show-recovery",
                            t!("Show the recovery key"),
                            ui::Variant::Primary,
                        )
                        .on_click(move |_, _, cx| {
                            showing.update(cx, |bench, cx| show_recovery(bench, cx))
                        }),
                    ),
                )
                .into_any_element();
            Some(warn_card(content))
        }
        (ShownStatus::Failed, _) => {
            let msg = shown
                .failed
                .clone()
                .unwrap_or_else(|| t!("Connector sync could not be reached.").into());
            let content = div()
                .flex()
                .flex_col()
                .gap(SPACE_1)
                .child(title(t!("Connector sync unavailable").into()))
                .child(desc(msg.into()))
                .into_any_element();
            Some(warn_card(content))
        }
        (ShownStatus::Off, _) | (ShownStatus::Open, None) => None,
    }
}

/// Quiet single-line Connector sync footer strip at the bottom of the Connectors page.
pub(super) fn footer(
    fields: &Fields,
    lit: bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Option<AnyElement> {
    let shown = fields.shown.as_ref()?;
    if shown.status == ShownStatus::Off {
        return None;
    }
    let t = Tokens::get(cx);
    let (tone, summary): (Tone, SharedString) = match shown.status {
        ShownStatus::Open => {
            let hosts = match shown.holders {
                0 => t!("Connector sync · sign-ins sealed to your account").to_owned(),
                1 => t!("Connector sync · sign-ins sealed to your account, on 1 host").to_owned(),
                many => t!(
                    "Connector sync · sign-ins sealed to your account, on {many} hosts",
                    many = many
                ),
            };
            (Tone::Ok, hosts.into())
        }
        ShownStatus::Waiting => (
            Tone::Pending,
            t!("Connector sync · waiting on this host’s recovery key").into(),
        ),
        ShownStatus::Failed => (
            Tone::Failed,
            t!("Connector sync · could not be reached").into(),
        ),
        ShownStatus::Off => return None,
    };
    let toggle_expand = (shown.status == ShownStatus::Open).then(|| {
        let toggling = this.clone();
        ui::Button::new(
            "connector-sync-manage",
            if fields.expanded {
                t!("Hide")
            } else {
                t!("Manage")
            },
            ui::Variant::Ghost,
        )
        .on_click(move |_, _, cx| {
            toggling.update(cx, |bench, cx| {
                change(bench, cx, |fields, _| fields.expanded = !fields.expanded)
            })
        })
    });
    let details = (shown.status == ShownStatus::Open && fields.expanded).then(|| {
        let switching = this.clone();
        div()
            .mt(SPACE_2)
            .mx(-SPACE_4)
            .rounded(theme::radius_2())
            .bg(t.surface_hover.opacity(0.35))
            .child(rows::row(
                t!("Keep new sign-ins on this host"),
                Some(t!(
                    "For a host you also use for work. Each new sign-in stays out of sync until you move it in."
                )),
                Switch::new("vault-keep-new")
                    .checked(shown.keep_new)
                    .on_change(move |on: &bool, _: &mut Window, cx: &mut App| {
                        let on = *on;
                        switching.update(cx, |bench, cx| keep_new(bench, on, cx))
                    }),
                cx,
            ))
    });
    Some(
        div()
            .flex()
            .flex_col()
            .pt(SPACE_3)
            .border_t(crate::theme::border())
            .border_color(t.border)
            .when(lit, |strip| {
                strip
                    .bg(t.surface_chosen)
                    .rounded(theme::radius_2())
                    .px(SPACE_2)
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .text_size(theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child(ui::dot(tone, cx))
                    .child(ui::icon(Icon::Shield, 13.).text_color(t.text_tertiary))
                    .child(div().flex_1().min_w_0().truncate().child(summary))
                    .children(toggle_expand),
            )
            .children(details)
            .into_any_element(),
    )
}

/// The ask under a connector before its sign-ins leave sync on a multi-host account.
pub(super) fn asking(
    plugin: &str,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> Option<AnyElement> {
    open_vault(fields)?;
    if fields.confirming.as_deref() != Some(plugin) {
        return None;
    }
    let t = Tokens::get(cx);
    let acting = this.clone();
    let cancelling = this.clone();
    let id = plugin.to_owned();
    Some(
        div()
            .flex()
            .items_center()
            .gap(SPACE_2)
            .pl(px(56.))
            .pr(SPACE_4)
            .py(SPACE_2)
            .border_b(crate::theme::border())
            .border_color(t.line_soft)
            .text_size(theme::ui_text(TEXT_2XS))
            .child(div().flex_1().min_w_0().text_color(t.warn).child(t!(
                "Every other host on the account loses this sign-in. It stays on this host alone."
            )))
            .child(
                ui::Button::new(
                    SharedString::from(format!("vault-keep-cancel-{plugin}")),
                    t!("Cancel"),
                    ui::Variant::Ghost,
                )
                .on_click(move |_, _, cx| {
                    cancelling.update(cx, |bench, cx| confirm(bench, None, cx))
                }),
            )
            .child(
                ui::Button::new(
                    SharedString::from(format!("vault-keep-sure-{plugin}")),
                    t!("Keep on this host only"),
                    ui::Variant::Primary,
                )
                .on_click(move |_, _, cx| {
                    acting.update(cx, |bench, cx| keep(bench, &id, true, cx))
                }),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use gpui::{size, Bounds, Modifiers, Pixels, Render, TestAppContext, VisualTestContext};
    use serde_json::{json, Value};
    use tokio::sync::mpsc;

    use super::*;
    use crate::core::{Ask, Link, Update};
    use crate::hosts::Place;
    use crate::reads::Read;
    use crate::settings::Page;

    struct Shown(Entity<Workbench>);

    impl Render for Shown {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.0
                .update(cx, |bench, cx| crate::settings::view(bench, window, cx))
        }
    }

    fn held(plugin: &str) -> ShownSignIn {
        ShownSignIn {
            plugin: plugin.into(),
            identity: "one".into(),
            label: "one".into(),
        }
    }

    struct Vaulted {
        vault: ShownVault,
        asks: mpsc::UnboundedReceiver<Ask>,
        updates: mpsc::UnboundedSender<Update>,
    }

    impl Vaulted {
        fn tell(&self) {
            let listed: Vec<Value> = ["kept", "shared", "last"]
                .map(|id| json!({ "id": id, "name": id, "enabled": true, "signedIn": true }))
                .into();
            let read = Update::Read(Place::This, Read::Plugins, json!(listed));
            self.updates.send(read).expect("the window listens");
        }

        fn answer(&mut self) -> Vec<(String, bool)> {
            let mut moved = Vec::new();
            while let Ok(ask) = self.asks.try_recv() {
                let Ask::Request(method, reply) = ask else {
                    continue;
                };
                if method.params_of::<call::Vault>().is_some() {
                    let _ = reply.send(Ok(json!(self.vault)));
                } else if let Some(keep) = method.params_of::<call::VaultKeep>() {
                    self.vault.kept.retain(|kept| kept.plugin != keep.plugin);
                    self.vault
                        .sign_ins
                        .retain(|kept| kept.plugin != keep.plugin);
                    match keep.kept {
                        true => self.vault.kept.push(held(&keep.plugin)),
                        false => self.vault.sign_ins.push(held(&keep.plugin)),
                    }
                    moved.push((keep.plugin, keep.kept));
                    let _ = reply.send(Ok(Value::Null));
                }
            }
            moved
        }
    }

    fn settle(core: &mut Vaulted, window: &mut VisualTestContext) -> Vec<(String, bool)> {
        let mut moved = Vec::new();
        for _ in 0..4 {
            window.update(|window, _| window.refresh());
            window.run_until_parked();
            let asked = core.answer();
            if !asked.is_empty() {
                core.tell();
            }
            moved.extend(asked);
        }
        moved
    }

    fn drawn(window: &mut VisualTestContext, id: &str) -> Vec<Bounds<Pixels>> {
        window.update(|window, _| {
            let mut found: Vec<_> = gpui_base::test_support::snapshots(window)
                .into_iter()
                .filter(|element| element.visible())
                .filter(|element| {
                    (element.path().iter()).any(|part| part.to_string().starts_with(id))
                })
                .map(|element| element.bounds())
                .collect();
            found.sort_by(|one, other| {
                f32::from(one.origin.y).total_cmp(&f32::from(other.origin.y))
            });
            found.dedup_by_key(|bounds| bounds.origin.y);
            found
        })
    }

    #[gpui::test]
    fn moving_a_kept_sign_in_redraws_its_one_line_row(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::install(cx);
        });
        let (updates, from_core) = mpsc::unbounded_channel();
        let (asks, asked) = mpsc::unbounded_channel();
        let link = Link {
            updates: from_core,
            asks,
        };
        let mut core = Vaulted {
            vault: ShownVault {
                status: ShownStatus::Open,
                holders: 1,
                kept: vec![held("kept")],
                sign_ins: vec![held("shared"), held("last")],
                ..ShownVault::default()
            },
            asks: asked,
            updates,
        };
        let (_, window) = cx.add_window_view(|window, cx| {
            let bench = cx.new(|cx| Workbench::new(link, window, cx));
            bench.update(cx, |bench, cx| {
                crate::settings::open(bench, Page::Connectors, window, cx)
            });
            let shown = cx.new(|_| Shown(bench));
            gpui_kit::base::Root::new(shown, window, cx)
        });
        window.simulate_resize(size(px(1400.), px(1000.)));
        core.tell();
        settle(&mut core, window);

        let rows: Vec<Pixels> = drawn(window, "connector-configure-")
            .iter()
            .map(|button| button.origin.y)
            .collect();
        assert_eq!(rows.len(), 3, "a row for each connector");
        assert_eq!(
            rows[1] - rows[0],
            rows[2] - rows[1],
            "the kept sign-in's row is as tall as the others'"
        );

        let configure_kept = drawn(window, "connector-configure-kept");
        assert_eq!(configure_kept.len(), 1);
        window.simulate_click(configure_kept[0].center(), Modifiers::none());
        settle(&mut core, window);

        let keep_switch = drawn(window, "connector-local-kept");
        assert_eq!(keep_switch.len(), 1, "inline configure shows local toggle");
        window.simulate_click(keep_switch[0].center(), Modifiers::none());
        let moved = settle(&mut core, window);
        assert_eq!(moved, [("kept".to_owned(), false)]);
        assert!(
            drawn(window, "connector-local-pill-kept").is_empty(),
            "the row dropped its THIS HOST pill after syncing"
        );
    }
}
