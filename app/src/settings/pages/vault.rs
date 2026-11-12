//! The account vault on the Connectors page: its recovery key once, a way back in with it,
//! and under each connector the sign-ins the vault holds, with the hosts that may use each,
//! which projects their agents may use it in, and its latest uses.

use std::collections::{BTreeSet, HashMap};

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, ClipboardItem, Context, Entity, FontWeight,
    InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled, Window,
};
use sikemux_core::server::call;
use sikemux_core::server::calls::NoParams;
use sikemux_core::server::vault::shown::{
    AllowParams, RecoverParams, RevokeParams, ShownHost, ShownSignIn, ShownStatus, ShownUse,
    ShownVault, UsesParams,
};

use crate::core::Target;
use crate::settings::rows::{self, Choice};
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS_H, TEXT_XS,
};
use crate::toast;
use crate::ui;
use crate::workbench::Workbench;

const EVERY_PROJECT: &str = "Every project";

/// What the page keeps of the vault while it shows.
#[derive(Default)]
pub struct Fields {
    shown: Option<ShownVault>,
    /// The sign-ins whose hosts are open.
    open: BTreeSet<String>,
    /// Each allowed host's project choice, by sign-in and host.
    scopes: HashMap<(String, String), Choice>,
    /// The sign-in whose use log is open, with what it read.
    log: Option<(String, Option<Vec<ShownUse>>)>,
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

fn project_names(bench: &Workbench) -> Vec<String> {
    let mut names: Vec<String> = bench
        .projects()
        .iter()
        .filter_map(|project| Some(project.path.file_name()?.to_string_lossy().into_owned()))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

/// Reads the vault, and makes the project choice of each host allowed a sign-in.
pub(super) fn read(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::Vault>(NoParams {});
    cx.spawn_in(window, async move |bench, cx| {
        let Ok(shown) = asked.await else { return };
        let _ = bench.update_in(cx, |bench, window, cx| {
            let projects = project_names(bench);
            let recovery = (shown.status == ShownStatus::Waiting).then(|| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .masked(true)
                        .placeholder("Recovery key")
                })
            });
            let mut scopes = HashMap::new();
            for sign_in in &shown.sign_ins {
                for host in sign_in.hosts.iter().filter(|host| host.allowed) {
                    let scope = scope_choice(sign_in, host, &projects, window, cx);
                    scopes.insert((sign_in.id.clone(), host.key.clone()), scope);
                }
            }
            change(bench, cx, |fields, _| {
                fields.shown = Some(shown);
                fields.scopes = scopes;
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

fn scope_choice(
    sign_in: &ShownSignIn,
    host: &ShownHost,
    projects: &[String],
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> Choice {
    let mut labels: Vec<String> = vec![EVERY_PROJECT.to_owned()];
    let chosen_label = match &host.projects {
        None => EVERY_PROJECT.to_owned(),
        Some(chosen) if chosen.len() == 1 => chosen[0].clone(),
        Some(chosen) => chosen.join(", "),
    };
    labels.extend(projects.iter().cloned());
    if !labels.contains(&chosen_label) {
        labels.push(chosen_label.clone());
    }
    let chosen = labels
        .iter()
        .position(|label| *label == chosen_label)
        .unwrap_or(0);
    let entry = sign_in.id.clone();
    let key = host.key.clone();
    let offered = labels.clone();
    Choice::new(
        labels,
        chosen,
        move |bench, at, cx| {
            let projects = match at {
                0 => None,
                at => match offered.get(at) {
                    Some(label) if projects_of(label).len() > 1 => return,
                    Some(label) => Some(vec![label.clone()]),
                    None => return,
                },
            };
            let asked = Target::here(&bench.asks).request_typed::<call::VaultAllow>(AllowParams {
                entry: entry.clone(),
                host: key.clone(),
                projects,
            });
            report(asked, "Project access changed", cx);
        },
        window,
        cx,
    )
}

fn projects_of(label: &str) -> Vec<&str> {
    label.split(", ").collect()
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

fn allow(bench: &Workbench, entry: &str, host: &str, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::VaultAllow>(AllowParams {
        entry: entry.to_owned(),
        host: host.to_owned(),
        projects: None,
    });
    report(asked, "Allowed", cx);
}

fn revoke(bench: &Workbench, entry: &str, host: &str, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::VaultRevoke>(RevokeParams {
        entry: entry.to_owned(),
        host: host.to_owned(),
    });
    report(asked, "Revoked", cx);
}

fn toggle_hosts(bench: &Workbench, id: &str, cx: &mut App) {
    let id = id.to_owned();
    change(bench, cx, |fields, _| {
        if !fields.open.remove(&id) {
            fields.open.insert(id);
        }
    });
}

fn toggle_log(bench: &mut Workbench, id: &str, cx: &mut Context<Workbench>) {
    let id = id.to_owned();
    let mut opening = false;
    change(bench, cx, |fields, _| {
        if fields.log.as_ref().is_some_and(|(open, _)| *open == id) {
            fields.log = None;
        } else {
            fields.log = Some((id.clone(), None));
            opening = true;
        }
    });
    if !opening {
        return;
    }
    let asked = Target::here(&bench.asks)
        .request_typed::<call::VaultUses>(UsesParams { entry: id.clone() });
    cx.spawn(async move |bench, cx| {
        let uses = asked.await.unwrap_or_default();
        let _ = bench.update(cx, |bench, cx| {
            change(bench, cx, |fields, _| {
                if let Some((open, read)) = &mut fields.log {
                    if *open == id {
                        *read = Some(uses);
                    }
                }
            });
        });
    })
    .detach();
}

fn saved_recovery(bench: &Workbench, cx: &mut Context<Workbench>) {
    let asked = Target::here(&bench.asks).request_typed::<call::VaultRecoverySaved>(NoParams {});
    report(asked, "Recovery key saved", cx);
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
        .rounded(theme::RADIUS_2)
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
        1 => Some("Sealed · 1 host holds the key".into()),
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
                .child(quiet("Save this recovery key somewhere only you can reach. It is shown once, and it is the only way back in if every host that holds the vault is lost.".into()))
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
        (ShownStatus::Open, None) => rows::note(
            t!("Sign-ins on this host move into the account's vault, sealed so the accounts server cannot read them. Each one says which hosts and projects may use it."),
            cx,
        ),
        (ShownStatus::Waiting, _) => {
            let restoring = this.clone();
            div()
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .px(SPACE_4)
                .py(SPACE_3)
                .child(quiet("This host waits for one that holds the vault's key to let it in. If none is left, type the recovery key you saved when the vault was made.".into()))
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
                .unwrap_or_else(|| "The account's vault could not be reached.".into()),
            cx,
        ),
    };
    Some(rows::section(
        t!("Account vault"),
        meta,
        None,
        lit,
        body,
        cx,
    ))
}

/// The sign-ins the vault holds for `plugin`, each under its connector's row.
pub(super) fn sign_ins(
    plugin: &str,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(shown) = fields
        .shown
        .as_ref()
        .filter(|shown| shown.status == ShownStatus::Open)
    else {
        return Vec::new();
    };
    shown
        .sign_ins
        .iter()
        .filter(|sign_in| sign_in.plugin == plugin)
        .map(|sign_in| sign_in_row(sign_in, fields, this, cx))
        .collect()
}

fn sign_in_row(
    sign_in: &ShownSignIn,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let open = fields.open.contains(&sign_in.id);
    let allowed = sign_in.hosts.iter().filter(|host| host.allowed).count();
    let summary = format!(
        "{} · from {} · allowed on {allowed} of {} hosts",
        sign_in.label,
        sign_in.origin,
        sign_in.hosts.len()
    );
    let toggling = this.clone();
    let id = sign_in.id.clone();
    let head = div()
        .flex()
        .items_center()
        .gap(SPACE_3)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(summary),
        )
        .child(
            ui::Button::new(
                SharedString::from(format!("vault-hosts-{}", sign_in.id)),
                if open { "Hide hosts" } else { "Hosts" },
                ui::Variant::Outline,
            )
            .on_click(move |_, _, cx| {
                toggling.update(cx, |bench, cx| toggle_hosts(bench, &id, cx))
            }),
        );
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .pl(px(60.))
        .pr(SPACE_4)
        .py(SPACE_2)
        .border_b_1()
        .border_color(t.line_soft)
        .child(head)
        .when(open, |rows| {
            rows.children(
                sign_in
                    .hosts
                    .iter()
                    .map(|host| host_row(sign_in, host, fields, this, cx)),
            )
            .child(last_use(sign_in, fields, this, cx))
        })
        .into_any_element()
}

fn host_row(
    sign_in: &ShownSignIn,
    host: &ShownHost,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let dot =
        div()
            .size(px(6.))
            .rounded_full()
            .bg(if host.online { t.live } else { t.text_tertiary });
    let mut notes = Vec::new();
    if host.this {
        notes.push("this host");
    }
    if host.refreshes {
        notes.push("refreshes it");
    }
    let name = div()
        .flex_1()
        .min_w_0()
        .flex()
        .gap(SPACE_1)
        .text_size(theme::ui_text(TEXT_XS))
        .child(
            div()
                .text_color(if host.allowed {
                    t.text_primary
                } else {
                    t.text_tertiary
                })
                .child(host.name.clone()),
        )
        .children((!notes.is_empty()).then(|| {
            div()
                .text_color(t.text_tertiary)
                .child(format!("· {}", notes.join(" · ")))
        }))
        .children((!host.allowed).then(|| div().text_color(t.warn).child("· waits for you")));
    let entry = sign_in.id.clone();
    let key = host.key.clone();
    let acting = this.clone();
    let action = if host.allowed {
        ui::Button::new(
            SharedString::from(format!("vault-revoke-{}-{}", sign_in.id, host.key)),
            "Revoke",
            ui::Variant::Outline,
        )
        .on_click(move |_, _, cx| acting.update(cx, |bench, cx| revoke(bench, &entry, &key, cx)))
    } else {
        ui::Button::new(
            SharedString::from(format!("vault-allow-{}-{}", sign_in.id, host.key)),
            "Allow",
            ui::Variant::Outline,
        )
        .on_click(move |_, _, cx| acting.update(cx, |bench, cx| allow(bench, &entry, &key, cx)))
    };
    let scope = fields
        .scopes
        .get(&(sign_in.id.clone(), host.key.clone()))
        .filter(|_| host.allowed)
        .map(Choice::view);
    div()
        .id(SharedString::from(format!(
            "vault-host-{}-{}",
            sign_in.id, host.key
        )))
        .flex()
        .items_center()
        .gap(SPACE_2)
        .py(SPACE_0)
        .child(dot)
        .child(name)
        .children(scope)
        .child(action)
        .into_any_element()
}

fn last_use(
    sign_in: &ShownSignIn,
    fields: &Fields,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let said = match &sign_in.last_use {
        Some(used) => format!(
            "Used {} by {} on {}",
            ui::time::ago(used.at),
            used.actor,
            used.host
        ),
        None => "Not used here lately".to_owned(),
    };
    let id = sign_in.id.clone();
    let opening = this.clone();
    let log = fields
        .log
        .as_ref()
        .filter(|(open, _)| *open == sign_in.id)
        .map(|(_, read)| match read {
            None => vec![rows::note(t!("Reading the use log…"), cx)],
            Some(uses) if uses.is_empty() => {
                vec![rows::note(t!("No use in the last 30 days."), cx)]
            }
            Some(uses) => uses
                .iter()
                .map(|used| {
                    div()
                        .text_size(theme::ui_text(TEXT_2XS_H))
                        .text_color(t.text_tertiary)
                        .child(format!(
                            "{} · {} on {} · {} {}",
                            ui::time::ago(used.at),
                            used.actor,
                            used.host,
                            used.count,
                            if used.count == 1 { "use" } else { "uses" }
                        ))
                        .into_any_element()
                })
                .collect(),
        })
        .unwrap_or_default();
    div()
        .flex()
        .flex_col()
        .gap(SPACE_1)
        .mt(SPACE_1)
        .pt(SPACE_1)
        .border_t_1()
        .border_color(t.line_soft)
        .child(
            div()
                .flex()
                .gap(SPACE_1)
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(said)
                .child("·")
                .child(
                    div()
                        .id(SharedString::from(format!("vault-log-{}", sign_in.id)))
                        .cursor_pointer()
                        .text_color(t.accent)
                        .font_weight(FontWeight::MEDIUM)
                        .child("Use log")
                        .on_click(move |_, _, cx| {
                            opening.update(cx, |bench, cx| toggle_log(bench, &id, cx))
                        }),
                ),
        )
        .children(log)
        .into_any_element()
}
