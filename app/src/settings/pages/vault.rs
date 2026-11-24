//! The account vault on the Connectors page: its recovery key once, or a way back in with it.
//! Every host on the account uses the sign-ins it holds, unless one is kept on this host alone,
//! out of the vault, and the host can make that the rule for new ones.

use gpui_kit::component::input::InputState;
use gpui_kit::component::switch::Switch;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClipboardItem, Context, Entity, IntoElement,
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
    /// The core told of a change that `shown` doesn't hold yet.
    pub(super) stale: bool,
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
        .border(crate::theme::border())
        .border_color(t.border)
        .bg(t.surface_sunken)
        .font_family(theme::mono_font())
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
        (ShownStatus::Open, None) if shown.recovery_in_window => {
            let showing = this.clone();
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .px(SPACE_4)
                .py(SPACE_3)
                .child(quiet(t!("Save the vault's recovery key somewhere only you can reach. It is the only way back in if every host that holds the vault is lost. Sikemux shows it in a window of its own, out of reach of your other programs.").into()))
                .child(
                    div().flex().child(
                        ui::Button::new("vault-show-recovery", t!("Show the recovery key"), ui::Variant::Primary)
                            .on_click(move |_, _, cx| {
                                showing.update(cx, |bench, cx| show_recovery(bench, cx))
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

/// Keep here only for a connector's sign-ins in the vault, Move into vault for those kept on
/// this host. A connector with some of each offers both.
pub(super) fn choices(plugin: &str, fields: &Fields, this: &Entity<Workbench>) -> Vec<ui::Button> {
    let Some(shown) = open_vault(fields) else {
        return Vec::new();
    };
    let others = shown.holders > 1;
    let keeping = of_plugin(&shown.sign_ins, plugin).next().map(|_| {
        let acting = this.clone();
        let id = plugin.to_owned();
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
        })
    });
    let moving = of_plugin(&shown.kept, plugin).next().map(|_| {
        let acting = this.clone();
        let id = plugin.to_owned();
        ui::Button::new(
            SharedString::from(format!("vault-share-{plugin}")),
            t!("Move into vault"),
            ui::Variant::Ghost,
        )
        .on_click(move |_, _, cx| acting.update(cx, |bench, cx| keep(bench, &id, false, cx)))
    });
    keeping.into_iter().chain(moving).collect()
}

/// The ask under a connector before its sign-ins leave the vault.
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
            .pl(px(60.))
            .pr(SPACE_4)
            .py(SPACE_2)
            .border_b(crate::theme::border())
            .border_color(t.line_soft)
            .text_size(theme::ui_text(TEXT_XS))
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

    /// Settings, drawn as the window draws them.
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

    /// A core that holds the vault, so far as the page asks of it.
    struct Vaulted {
        vault: ShownVault,
        asks: mpsc::UnboundedReceiver<Ask>,
        updates: mpsc::UnboundedSender<Update>,
    }

    impl Vaulted {
        /// Sends the connectors, as the core does when they or the vault change.
        fn tell(&self) {
            let listed: Vec<Value> = ["kept", "shared", "last"]
                .map(|id| json!({ "id": id, "name": id, "enabled": true, "signedIn": true }))
                .into();
            let read = Update::Read(Place::This, Read::Plugins, json!(listed));
            self.updates.send(read).expect("the window listens");
        }

        /// Answers what the page asked, and says how each sign-in it moved is now kept.
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

    /// Draws the page until neither it nor the core has more to say, and gives what it moved.
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

    /// Where each control with this name is drawn, top to bottom.
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

        let rows: Vec<Pixels> = drawn(window, "plugin-on-")
            .iter()
            .map(|switch| switch.origin.y)
            .collect();
        assert_eq!(rows.len(), 3, "a row for each connector");
        assert_eq!(
            rows[1] - rows[0],
            rows[2] - rows[1],
            "the kept sign-in's row is as tall as the others'"
        );
        let moving = drawn(window, "vault-share-kept");
        assert_eq!(moving.len(), 1, "the kept sign-in offers Move into vault");
        assert!(drawn(window, "vault-keep-kept").is_empty());

        window.simulate_click(moving[0].center(), Modifiers::none());
        let moved = settle(&mut core, window);
        assert_eq!(moved, [("kept".to_owned(), false)]);
        assert!(
            drawn(window, "vault-share-").is_empty(),
            "the row still offers the move it made"
        );
        assert_eq!(drawn(window, "vault-keep-kept").len(), 1);
    }
}
