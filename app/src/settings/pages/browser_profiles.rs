//! Settings → Browser profiles (Option A — Quiet inline): manage standalone browser profiles,
//! global default, agent descriptions, access guardrails, usage inspection, site-data clearing,
//! and safe profile deletion.

use std::collections::HashMap;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, SharedString, Styled, Subscription, Window,
};
use sikemux_calls::browser_profiles::BrowserProfileUpsertParams;
use sikemux_view::browser_profiles::{BrowserProfile, ProfileAccess};

use crate::icons::Icon;
use crate::settings::index::Page::BrowserProfiles;
use crate::settings::index::{row, section, Entry};
use crate::settings::rows;
use crate::settings::Settings;
use crate::theme::{
    self, Tokens, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS, TEXT_2XS_H, TEXT_3XS,
    TEXT_3XS_H, TEXT_SM, TEXT_XS,
};
use crate::ui::{self, dialog, Button, Clicks, IconButton, Variant};
use crate::workbench::Workbench;

pub(in crate::settings) const ENTRIES: &[Entry] = &[
    section(
        BrowserProfiles,
        "Default",
        "browser profiles global default cookies sessions sign in",
    ),
    row(
        BrowserProfiles,
        "Default",
        "Global default",
        "browser profile default fallback",
    ),
    section(
        BrowserProfiles,
        "Profiles",
        "browser profiles create rename delete clear site data cookies spaces playbook",
    ),
];

#[derive(PartialEq)]
struct GlobalOffer {
    ids: Vec<String>,
    labels: Vec<SharedString>,
    chosen: usize,
}

impl GlobalOffer {
    fn of(bench: &Workbench) -> Self {
        let profiles = bench.browser_profiles();
        let global = bench.global_default_browser_profile();
        let chosen = profiles.iter().position(|p| p.id == global.id).unwrap_or(0);
        Self {
            ids: profiles.iter().map(|p| p.id.clone()).collect(),
            labels: profiles.iter().map(|p| p.name.clone().into()).collect(),
            chosen,
        }
    }
}

pub struct Fields {
    global_choice: rows::Choice,
    global_offer: GlobalOffer,
    selected: Option<String>,
    renaming: Option<String>,
    rename_input: Option<(Entity<InputState>, Subscription)>,
    desc_inputs: HashMap<String, (Entity<InputState>, Subscription)>,
    confirm_delete: Option<String>,
}

pub(super) fn stale(fields: &Fields, bench: &Workbench) -> bool {
    fields.global_offer != GlobalOffer::of(bench)
}

fn global_choice(
    bench: &Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (rows::Choice, GlobalOffer) {
    let offer = GlobalOffer::of(bench);
    let ids = offer.ids.clone();
    let choice = rows::Choice::new(
        offer.labels.clone(),
        offer.chosen,
        move |bench, at, cx| {
            if let Some(id) = ids.get(at) {
                bench.set_global_browser_profile(id.clone(), cx);
            }
        },
        window,
        cx,
    );
    (choice, offer)
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let (global_choice, global_offer) = global_choice(bench, window, cx);
    let selected = bench.browser_profiles().first().map(|p| p.id.clone());
    let mut fields = Fields {
        global_choice,
        global_offer,
        selected,
        renaming: None,
        rename_input: None,
        desc_inputs: HashMap::new(),
        confirm_delete: None,
    };
    sync_desc_inputs(&mut fields, bench, window, cx);
    super::change_pages(bench, cx, |pages, _| pages.browser_profiles = Some(fields));
}

pub(super) fn rebuild_choices(
    bench: &mut Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let (choice, offer) = global_choice(bench, window, cx);
    let profiles = bench.browser_profiles();
    let mut new_inputs: HashMap<String, (Entity<InputState>, Subscription)> = HashMap::new();
    let existing_keys: Vec<String> = super::state(bench, cx)
        .and_then(|pages| pages.browser_profiles.as_ref())
        .map(|fields| fields.desc_inputs.keys().cloned().collect())
        .unwrap_or_default();
    for profile in &profiles {
        if !existing_keys.contains(&profile.id) {
            new_inputs.insert(profile.id.clone(), make_desc_input(profile, window, cx));
        }
    }
    change_fields(bench, cx, |fields, _| {
        fields.global_choice = choice;
        fields.global_offer = offer;
        fields
            .desc_inputs
            .retain(|id, _| profiles.iter().any(|p| &p.id == id));
        for (id, pair) in new_inputs {
            fields.desc_inputs.insert(id, pair);
        }
        if fields
            .selected
            .as_deref()
            .is_some_and(|id| !profiles.iter().any(|p| p.id == id))
        {
            fields.selected = profiles.first().map(|p| p.id.clone());
        }
    });
}

fn sync_desc_inputs(
    fields: &mut Fields,
    bench: &Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let profiles = bench.browser_profiles();
    for profile in &profiles {
        if !fields.desc_inputs.contains_key(&profile.id) {
            fields
                .desc_inputs
                .insert(profile.id.clone(), make_desc_input(profile, window, cx));
        }
    }
}

fn make_desc_input(
    profile: &BrowserProfile,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t!("e.g. Team SaaS tools, Jira, staging — or leave empty"))
            .default_value(profile.description.clone())
    });
    let profile_id = profile.id.clone();
    let sub = cx.subscribe_in(&input, window, move |bench, field, event, _, cx| {
        if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
            let typed = field.read(cx).value().trim().to_owned();
            let Some(current) = bench.browser_profile_by_id(&profile_id) else {
                return;
            };
            if current.description == typed {
                return;
            }
            bench.upsert_browser_profile(
                BrowserProfileUpsertParams {
                    id: Some(current.id),
                    name: current.name,
                    slug: Some(current.slug),
                    description: Some(typed),
                    access: Some(current.access),
                    link_space_id: None,
                },
                Some(t!("Profile description saved").to_owned()),
                None,
                cx,
            );
        }
    });
    (input, sub)
}

fn start_rename(
    bench: &mut Workbench,
    profile_id: String,
    initial_name: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    if super::state(bench, cx).is_some_and(|pages| pages.browser_profiles.is_none()) {
        make(bench, window, cx);
    }
    let current_name = bench
        .browser_profile_by_id(&profile_id)
        .map(|p| p.name)
        .or(initial_name)
        .unwrap_or_default();
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t!("Profile name"))
            .default_value(current_name)
    });
    let pid = profile_id.clone();
    let sub = cx.subscribe_in(&input, window, move |bench, field, event, _, cx| {
        if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
            let typed = field.read(cx).value().trim().to_owned();
            change_fields(bench, cx, |fields, _| {
                fields.renaming = None;
                fields.rename_input = None;
            });
            if typed.is_empty() {
                return;
            }
            let Some(current) = bench.browser_profile_by_id(&pid) else {
                return;
            };
            if current.name == typed {
                return;
            }
            let next_name = typed.clone();
            bench.upsert_browser_profile(
                BrowserProfileUpsertParams {
                    id: Some(current.id),
                    name: typed,
                    slug: None,
                    description: Some(current.description),
                    access: Some(current.access),
                    link_space_id: None,
                },
                Some(format!("Renamed to “{next_name}”")),
                None,
                cx,
            );
        }
    });
    input.update(cx, |input, cx| {
        input.focus(window, cx);
        input.select_all(window, cx);
    });
    change_fields(bench, cx, |fields, _| {
        fields.selected = Some(profile_id.clone());
        fields.renaming = Some(profile_id);
        fields.rename_input = Some((input, sub));
    });
}

pub(in crate::settings) fn create_profile(
    bench: &mut Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let window_handle = window.window_handle();
    let count = bench.browser_profiles().len();
    let name = format!("Profile {}", count + 1);
    bench.upsert_browser_profile(
        BrowserProfileUpsertParams {
            id: None,
            name,
            slug: None,
            description: Some(String::new()),
            access: Some(ProfileAccess::Standard),
            link_space_id: None,
        },
        Some(t!("New browser profile created").to_owned()),
        Some(Box::new(move |bench, created, cx| {
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = bench.update(cx, |bench, cx| {
                    start_rename(bench, created.id, Some(created.name), window, cx);
                });
            });
        })),
        cx,
    );
}

fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.browser_profiles.as_mut(), change)
}

struct Usage {
    spaces: Vec<(String, String)>,
    pins: Vec<String>,
    tabs: Vec<(String, Option<String>)>,
}

fn profile_usage(bench: &Workbench, profile_id: &str, cx: &App) -> Usage {
    let view = bench.this_view();
    let spaces = view
        .map(|view| {
            view.workspace
                .spaces
                .iter()
                .filter(|space| space.browser_profile.as_deref() == Some(profile_id))
                .map(|space| (space.name.clone(), space.icon.clone()))
                .collect()
        })
        .unwrap_or_default();
    let pins_map = &bench.preferences().project_browser_profiles;
    let pins = pins_map
        .iter()
        .filter(|(_, pid)| pid.as_str() == profile_id)
        .map(|(folder, _)| {
            view.and_then(|view| {
                view.workspace
                    .projects
                    .iter()
                    .find(|p| p.path.to_string_lossy() == *folder)
                    .map(|p| p.name.clone())
            })
            .unwrap_or_else(|| {
                std::path::Path::new(folder)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| folder.clone())
            })
        })
        .collect();
    let tabs = bench
        .desks
        .read(cx)
        .engine
        .as_ref()
        .map(|engine| {
            engine
                .overridden_tabs_for_profile(profile_id)
                .into_iter()
                .map(|(_, _, title, by)| (title, by))
                .collect()
        })
        .unwrap_or_default();
    Usage { spaces, pins, tabs }
}

fn usage_summary(usage: &Usage, is_global: bool) -> String {
    let mut bits = Vec::new();
    if !usage.spaces.is_empty() {
        let names: Vec<&str> = usage.spaces.iter().map(|(name, _)| name.as_str()).collect();
        bits.push(format!("Space {}", names.join(", ")));
    }
    if !usage.pins.is_empty() {
        let n = usage.pins.len();
        bits.push(format!("{n} Playbook pin{}", if n == 1 { "" } else { "s" }));
    }
    if !usage.tabs.is_empty() {
        let n = usage.tabs.len();
        bits.push(format!("{n} tab{}", if n == 1 { "" } else { "s" }));
    }
    if is_global {
        bits.push("everything else".to_owned());
    }
    if bits.is_empty() {
        "Not used yet".to_owned()
    } else {
        bits.join(" · ")
    }
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.browser_profiles else {
        return Vec::new();
    };
    let t = Tokens::get(cx).clone();
    let profiles = bench.browser_profiles();
    let global = bench.global_default_browser_profile();

    let default_section = rows::section(
        t!("Default"),
        None,
        None,
        lit("Default"),
        rows::row(
            t!("Global default"),
            Some(t!("When nothing else picks one")),
            fields.global_choice.view(),
            cx,
        ),
        cx,
    );

    let profile_rows: Vec<AnyElement> = profiles
        .iter()
        .map(|profile| {
            let pid = profile.id.clone();
            let is_global = profile.id == global.id;
            let is_selected = fields.selected.as_deref() == Some(profile.id.as_str());
            let is_renaming = fields.renaming.as_deref() == Some(profile.id.as_str());
            let usage = profile_usage(bench, &profile.id, cx);
            let summary = usage_summary(&usage, is_global);
            let row_id = SharedString::from(format!("bp-row-{}", profile.id));

            let name_el: AnyElement = if is_renaming {
                if let Some((input, _)) = &fields.rename_input {
                    div()
                        .w(px(170.))
                        .child(Input::new(input).small())
                        .into_any_element()
                } else {
                    div().child(profile.name.clone()).into_any_element()
                }
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(theme::ui_text(TEXT_SM))
                    .text_color(t.text_primary)
                    .child(ui::icon(Icon::User, 13.).text_color(t.text_tertiary))
                    .child(profile.name.clone())
                    .into_any_element()
            };

            let (sel_this, sel_pid) = (this.clone(), pid.clone());
            let (ren_this, ren_pid) = (this.clone(), pid.clone());
            let (def_this, def_pid) = (this.clone(), pid.clone());
            let (del_this, del_pid) = (this.clone(), pid.clone());

            let header = div()
                .id(row_id.clone())
                .group(row_id.clone())
                .min_h(px(40.))
                .px(SPACE_4)
                .py(SPACE_1H)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .border_b(theme::border())
                .border_color(t.line_soft)
                .cursor_pointer()
                .when(is_selected, |row| row.bg(t.surface_chosen))
                .when(!is_selected, |row| {
                    row.hover(move |row| row.bg(t.surface_hover))
                })
                .when(!is_renaming, |row| {
                    row.clicks(move |_, _, cx| {
                        let pid = sel_pid.clone();
                        sel_this.update(cx, |bench, cx| {
                            change_fields(bench, cx, |fields, _| {
                                if fields.selected.as_deref() == Some(pid.as_str()) {
                                    fields.selected = None;
                                } else {
                                    fields.selected = Some(pid);
                                }
                            });
                        });
                    })
                })
                .child(name_el)
                .when(is_global, |row| {
                    row.child(
                        div()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(theme::radius_1())
                            .border(theme::border())
                            .border_color(t.border_selected)
                            .text_size(theme::ui_text(TEXT_3XS))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.accent)
                            .child(t!("DEFAULT")),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(theme::ui_text(TEXT_2XS_H))
                        .text_color(t.text_tertiary)
                        .child(summary),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .opacity(if is_selected { 1. } else { 0. })
                        .group_hover(row_id, |acts| acts.opacity(1.))
                        .child(
                            IconButton::new(
                                SharedString::from(format!("bp-rename-{}", profile.id)),
                                Icon::Pencil,
                                t!("Rename"),
                            )
                            .small()
                            .on_click(move |_, window, cx| {
                                let pid = ren_pid.clone();
                                ren_this.update(cx, |bench, cx| {
                                    start_rename(bench, pid, None, window, cx)
                                });
                            }),
                        )
                        .when(!is_global, |acts| {
                            acts.child(
                                IconButton::new(
                                    SharedString::from(format!("bp-default-{}", profile.id)),
                                    Icon::Check,
                                    t!("Make default"),
                                )
                                .small()
                                .on_click(move |_, _, cx| {
                                    let pid = def_pid.clone();
                                    def_this.update(cx, |bench, cx| {
                                        bench.set_global_browser_profile(pid, cx);
                                    });
                                }),
                            )
                        })
                        .when(!profile.is_default_store, |acts| {
                            acts.child(
                                IconButton::new(
                                    SharedString::from(format!("bp-delete-{}", profile.id)),
                                    Icon::Trash,
                                    t!("Delete profile"),
                                )
                                .small()
                                .on_click(move |_, _, cx| {
                                    let pid = del_pid.clone();
                                    del_this.update(cx, |bench, cx| {
                                        change_fields(bench, cx, |fields, _| {
                                            fields.confirm_delete = Some(pid);
                                        });
                                    });
                                }),
                            )
                        }),
                );

            if !is_selected {
                return header.into_any_element();
            }

            let desc_field = fields.desc_inputs.get(&profile.id).map(|(input, _)| {
                div()
                    .w_full()
                    .child(Input::new(input).small())
                    .into_any_element()
            });

            let access_buttons = [
                ProfileAccess::Standard,
                ProfileAccess::AskOutsideProject,
                ProfileAccess::ManualOnly,
            ]
            .into_iter()
            .map(|policy| {
                let on = profile.access == policy;
                let acc_this = this.clone();
                let current = profile.clone();
                let label = crate::browser_profiles::access_label(policy);
                div()
                    .id(SharedString::from(format!(
                        "bp-access-{}-{}",
                        profile.id, label
                    )))
                    .h(px(24.))
                    .px(SPACE_2)
                    .flex()
                    .items_center()
                    .rounded(theme::radius_2())
                    .cursor_pointer()
                    .text_size(theme::ui_text(TEXT_2XS))
                    .when(on, |btn| {
                        btn.bg(t.surface_chosen)
                            .text_color(t.text_primary)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!on, |btn| {
                        btn.text_color(t.text_tertiary)
                            .hover(move |btn| btn.bg(t.surface_hover).text_color(t.text_primary))
                    })
                    .clicks(move |_, _, cx| {
                        let current = current.clone();
                        acc_this.update(cx, |bench, cx| {
                            bench.upsert_browser_profile(
                                BrowserProfileUpsertParams {
                                    id: Some(current.id),
                                    name: current.name,
                                    slug: Some(current.slug),
                                    description: Some(current.description),
                                    access: Some(policy),
                                    link_space_id: None,
                                },
                                Some(format!("Agent access set to {label}")),
                                None,
                                cx,
                            );
                        });
                    })
                    .child(label)
            });

            let mut used_rows: Vec<AnyElement> = Vec::new();
            for (space_name, space_icon) in &usage.spaces {
                let glyph = Icon::named(space_icon).unwrap_or(Icon::Folder);
                used_rows.push(
                    div()
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(ui::icon(glyph, 12.).text_color(t.text_tertiary))
                        .child(format!("Space {space_name}"))
                        .child(
                            div()
                                .ml_auto()
                                .text_size(theme::ui_text(TEXT_2XS_H))
                                .text_color(t.text_tertiary)
                                .child(t!("default")),
                        )
                        .into_any_element(),
                );
            }
            for project_name in &usage.pins {
                used_rows.push(
                    div()
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(ui::icon(Icon::Pin, 12.).text_color(t.text_tertiary))
                        .child(project_name.clone())
                        .child(
                            div()
                                .ml_auto()
                                .text_size(theme::ui_text(TEXT_2XS_H))
                                .text_color(t.text_tertiary)
                                .child(t!("Playbook pin")),
                        )
                        .into_any_element(),
                );
            }
            for (tab_title, by) in &usage.tabs {
                let glyph = if by.is_some() {
                    Icon::Agent
                } else {
                    Icon::Globe
                };
                let tag = match by {
                    Some(who) => format!("{who} override"),
                    None => t!("tab override").to_owned(),
                };
                used_rows.push(
                    div()
                        .h(px(22.))
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_secondary)
                        .child(ui::icon(glyph, 12.).text_color(t.text_tertiary))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(tab_title.clone()),
                        )
                        .child(
                            div()
                                .ml_auto()
                                .text_size(theme::ui_text(TEXT_2XS_H))
                                .text_color(t.text_tertiary)
                                .child(tag),
                        )
                        .into_any_element(),
                );
            }

            let (clear_this, clear_pid) = (this.clone(), profile.id.clone());
            let (del_btn_this, del_btn_pid) = (this.clone(), profile.id.clone());

            let detail = div()
                .px(SPACE_4)
                .py(SPACE_3)
                .flex()
                .flex_col()
                .gap(SPACE_3)
                .border_b(theme::border())
                .border_color(t.line_soft)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACE_1)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_size(theme::ui_text(TEXT_2XS_H))
                                        .text_color(t.text_tertiary)
                                        .child(t!("What agents read when choosing a profile")),
                                )
                                .child(ui::mono(format!("slug: {}", profile.slug), TEXT_3XS, cx)),
                        )
                        .children(desc_field),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(SPACE_2)
                        .child(
                            div()
                                .text_size(theme::ui_text(TEXT_2XS_H))
                                .text_color(t.text_tertiary)
                                .child(t!("Agent access")),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(2.))
                                .p(px(2.))
                                .rounded(theme::radius_2())
                                .border(theme::border())
                                .border_color(t.border)
                                .bg(t.surface_sunken)
                                .children(access_buttons),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(SPACE_1)
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(SPACE_2)
                                .child(ui::caps(t!("Used by"), cx))
                                .child(div().flex_1().h(px(1.)).bg(t.border)),
                        )
                        .map(|col| {
                            if used_rows.is_empty() {
                                col.child(
                                    div()
                                        .text_size(theme::ui_text(TEXT_2XS_H))
                                        .text_color(t.text_tertiary)
                                        .child(t!(
                                            "Nothing points here. Tabs reach it only when picked by hand."
                                        )),
                                )
                            } else {
                                col.children(used_rows)
                            }
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_2)
                        .child(
                            Button::new(
                                SharedString::from(format!("bp-clear-{}", profile.id)),
                                t!("Clear site data"),
                                Variant::Outline,
                            )
                            .sized(26., 10., TEXT_2XS)
                            .on_click(move |_, _, cx| {
                                let pid = clear_pid.clone();
                                clear_this.update(cx, |bench, cx| {
                                    bench.clear_browser_profile_data(&pid, cx);
                                });
                            }),
                        )
                        .when(!profile.is_default_store, |bar| {
                            bar.child(
                                Button::new(
                                    SharedString::from(format!("bp-del-detail-{}", profile.id)),
                                    t!("Delete profile"),
                                    Variant::Outline,
                                )
                                .icon(Icon::Trash)
                                .sized(26., 10., TEXT_2XS)
                                .on_click(move |_, _, cx| {
                                    let pid = del_btn_pid.clone();
                                    del_btn_this.update(cx, |bench, cx| {
                                        change_fields(bench, cx, |fields, _| {
                                            fields.confirm_delete = Some(pid);
                                        });
                                    });
                                }),
                            )
                        })
                        .when(profile.is_default_store, |bar| {
                            bar.child(
                                div()
                                    .text_size(theme::ui_text(TEXT_3XS_H))
                                    .text_color(t.text_tertiary)
                                    .child(t!("Default store · cannot be deleted")),
                            )
                        }),
                );

            div()
                .flex()
                .flex_col()
                .child(header)
                .child(detail)
                .into_any_element()
        })
        .collect();

    let new_this = this.clone();
    let footer = div().px(SPACE_4).py(SPACE_2).child(
        Button::new("bp-new-profile", t!("New profile"), Variant::Ghost)
            .icon(Icon::Plus)
            .sized(26., 8., TEXT_XS)
            .on_click(move |_, window, cx| {
                new_this.update(cx, |bench, cx| create_profile(bench, window, cx));
            }),
    );

    let profiles_section = rows::section(
        t!("Profiles"),
        Some(format!("{}", profiles.len()).into()),
        None,
        lit("Profiles"),
        div().flex().flex_col().children(profile_rows).child(footer),
        cx,
    );

    vec![default_section, profiles_section]
}

pub(super) fn dialog(bench: &Workbench, this: &Entity<Workbench>, cx: &App) -> Option<AnyElement> {
    let delete_id = super::state(bench, cx)
        .and_then(|pages| pages.browser_profiles.as_ref())
        .and_then(|fields| fields.confirm_delete.as_ref())?
        .clone();
    let profile = bench.browser_profile_by_id(&delete_id)?;
    let profiles = bench.browser_profiles();
    let global = bench.global_default_browser_profile();
    let is_global = global.id == profile.id;
    let next_global = profiles.iter().find(|p| p.id != profile.id);
    let usage = profile_usage(bench, &profile.id, cx);
    let t = Tokens::get(cx).clone();

    let mut bullets: Vec<(Icon, String)> = vec![(
        Icon::Trash,
        t!("Its cookies and sign-ins are erased. Nothing else is touched.").to_owned(),
    )];
    for (space_name, _) in &usage.spaces {
        bullets.push((
            Icon::Folder,
            format!("Space {space_name} stays, and falls back to the global default."),
        ));
    }
    for project_name in &usage.pins {
        bullets.push((
            Icon::Pin,
            format!("{project_name}'s pin is dropped; it follows its space again."),
        ));
    }
    if !usage.tabs.is_empty() {
        let n = usage.tabs.len();
        bullets.push((
            Icon::Globe,
            format!(
                "{n} open tab{} reload in what they inherit.",
                if n == 1 { "" } else { "s" }
            ),
        ));
    }
    if is_global {
        if let Some(next) = next_global {
            bullets.push((
                Icon::InfoCircle,
                format!("It's the global default; {} takes over.", next.name),
            ));
        }
    }

    let body = div()
        .flex()
        .flex_col()
        .gap(SPACE_2)
        .child(
            div()
                .text_size(theme::ui_text(TEXT_2XS_H))
                .text_color(t.text_tertiary)
                .child(t!("Can't be undone.")),
        )
        .children(bullets.into_iter().map(|(icon, text)| {
            div()
                .flex()
                .items_start()
                .gap(SPACE_2)
                .text_size(theme::ui_text(TEXT_XS))
                .text_color(t.text_secondary)
                .child(
                    div()
                        .pt(px(2.))
                        .child(ui::icon(icon, 13.).text_color(t.text_tertiary)),
                )
                .child(div().flex_1().min_w_0().child(text))
        }));

    let close_this = this.clone();
    let confirm_this = this.clone();
    let pid = profile.id.clone();

    let no: dialog::Answer = std::rc::Rc::new(move |_, cx| {
        close_this.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| fields.confirm_delete = None);
        });
    });
    let yes: dialog::Answer = std::rc::Rc::new(move |_, cx| {
        let pid = pid.clone();
        confirm_this.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| fields.confirm_delete = None);
            bench.delete_browser_profile(pid, cx);
        });
    });

    let cancel_act = no.clone();
    let delete_act = yes.clone();

    Some(
        dialog::sheet(
            format!("Delete “{}”?", profile.name).into(),
            body.into_any_element(),
            div()
                .flex()
                .justify_end()
                .gap(SPACE_1H)
                .child(
                    Button::new("bp-del-cancel", t!("Cancel"), Variant::Outline)
                        .on_click(move |_, window, cx| cancel_act(window, cx)),
                )
                .child(
                    Button::new("bp-del-confirm", t!("Delete profile"), Variant::Primary)
                        .solid(t.danger, t.danger.opacity(0.88))
                        .on_click(move |_, window, cx| delete_act(window, cx)),
                ),
            yes,
            no,
            cx,
        )
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use gpui::{size, Bounds, Modifiers, Pixels, Render, TestAppContext, VisualTestContext};
    use serde_json::json;
    use sikemux_calls::browser_profiles::BrowserProfileUpsertAnswer;
    use sikemux_calls::call;
    use sikemux_view::browser_profiles::{default_profile, DEFAULT_PROFILE_ID};
    use sikemux_view::prefs::Preferences;
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

    fn drawn(window: &mut VisualTestContext, id: &str) -> Vec<Bounds<Pixels>> {
        window.update(|window, _| {
            gpui_base::test_support::snapshots(window)
                .into_iter()
                .filter(|element| element.visible())
                .filter(|element| element.path().iter().any(|part| part.to_string() == id))
                .map(|element| element.bounds())
                .collect()
        })
    }

    fn settle(window: &mut VisualTestContext) {
        for _ in 0..3 {
            window.update(|window, _| window.refresh());
            window.run_until_parked();
        }
    }

    #[gpui::test]
    fn clicking_new_profile_enters_inline_rename_without_panicking(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            theme::install(cx);
        });
        let (updates, from_core) = mpsc::unbounded_channel();
        let (asks, mut asked) = mpsc::unbounded_channel();
        let link = Link {
            updates: from_core,
            asks,
        };
        let mut profiles = vec![
            default_profile(),
            BrowserProfile {
                id: "profile-2".into(),
                slug: "profile-2".into(),
                name: "Profile 2".into(),
                description: String::new(),
                access: ProfileAccess::Standard,
                is_default_store: false,
                created_at: 1,
            },
        ];
        let mut bench_handle = None;
        let (_, window) = cx.add_window_view(|window, cx| {
            let bench = cx.new(|cx| Workbench::new(link, window, cx));
            bench.update(cx, |bench, cx| {
                crate::settings::open(bench, Page::BrowserProfiles, window, cx)
            });
            bench_handle = Some(bench.clone());
            let shown = cx.new(|_| Shown(bench));
            gpui_kit::base::Root::new(shown, window, cx)
        });
        let bench = bench_handle.expect("workbench created");
        window.simulate_resize(size(px(1400.), px(1000.)));

        let send_prefs = |profiles: &[BrowserProfile]| {
            let prefs = Preferences {
                browser_profiles: profiles.to_vec(),
                ..Preferences::default()
            };
            updates
                .send(Update::Read(Place::This, Read::Preferences, json!(prefs)))
                .expect("window listens");
        };
        send_prefs(&profiles);
        settle(window);

        let new_btn = drawn(window, "bp-new-profile");
        assert_eq!(new_btn.len(), 1, "New profile button is visible");
        window.simulate_click(new_btn[0].center(), Modifiers::none());
        settle(window);

        let mut created = None;
        while let Ok(ask) = asked.try_recv() {
            let Ask::Request(method, reply) = ask else {
                continue;
            };
            if let Some(params) = method.params_of::<call::BrowserProfileUpsert>() {
                assert_eq!(params.name, "Profile 3");
                let profile = BrowserProfile {
                    id: "profile-3".into(),
                    slug: "profile-3".into(),
                    name: params.name,
                    description: params.description.unwrap_or_default(),
                    access: params.access.unwrap_or_default(),
                    is_default_store: false,
                    created_at: 2,
                };
                profiles.push(profile.clone());
                created = Some(profile.clone());
                let answer = BrowserProfileUpsertAnswer {
                    profile,
                    profiles: profiles.clone(),
                    global_default_id: DEFAULT_PROFILE_ID.into(),
                };
                let _ = reply.send(Ok(json!(answer)));
            }
        }
        assert!(created.is_some(), "upsert request was sent");
        settle(window);

        window.update(|_, cx| {
            let bench = bench.read(cx);
            let fields = super::super::state(bench, cx)
                .and_then(|pages| pages.browser_profiles.as_ref())
                .expect("browser profiles page initialized");
            assert_eq!(fields.selected.as_deref(), Some("profile-3"));
            assert_eq!(fields.renaming.as_deref(), Some("profile-3"));
            let (input, _) = fields.rename_input.as_ref().expect("rename input created");
            assert_eq!(input.read(cx).value().as_str(), "Profile 3");
        });

        send_prefs(&profiles);
        settle(window);

        window.update(|_, cx| {
            let bench = bench.read(cx);
            let fields = super::super::state(bench, cx)
                .and_then(|pages| pages.browser_profiles.as_ref())
                .expect("browser profiles page initialized");
            assert_eq!(fields.selected.as_deref(), Some("profile-3"));
            assert_eq!(fields.renaming.as_deref(), Some("profile-3"));
        });
    }
}
