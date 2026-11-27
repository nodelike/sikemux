//! Settings → Browser profiles (Settings A — strictly flat one-sheet expandable rows):
//! manage standalone browser profiles, global default, inline name/note/access editing,
//! usage inspection, site-data clearing, and safe profile deletion.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, AppContext, Context, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Radians, SharedString, Styled, Subscription, Transformation,
    Window,
};
use sikemux_calls::browser_profiles::BrowserProfileUpsertParams;
use sikemux_view::browser_profiles::{BrowserProfile, ProfileAccess};

use crate::browser_profiles::{
    access_description, access_label, access_segmented, flat_input, profile_mark,
    profile_picker_trigger, NewProfileTarget, PickerMode,
};
use crate::icons::Icon;
use crate::settings::index::Page::BrowserProfiles;
use crate::settings::index::{row, section, Entry};
use crate::settings::rows;
use crate::settings::Settings;
use crate::theme::{
    self, radius_1, radius_2, Tokens, SPACE_1H, SPACE_2, SPACE_3, SPACE_4, TEXT_2XS_H, TEXT_3XS_H,
    TEXT_MD, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::toast;
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
struct ProfileSnapshot {
    ids: Vec<String>,
    names: Vec<String>,
    descriptions: Vec<String>,
}

impl ProfileSnapshot {
    fn of(bench: &Workbench) -> Self {
        let profiles = bench.browser_profiles();
        Self {
            ids: profiles.iter().map(|p| p.id.clone()).collect(),
            names: profiles.iter().map(|p| p.name.clone()).collect(),
            descriptions: profiles.iter().map(|p| p.description.clone()).collect(),
        }
    }
}

pub struct Fields {
    snapshot: ProfileSnapshot,
    selected: Option<String>,
    name_inputs: HashMap<String, (Entity<InputState>, Subscription)>,
    desc_inputs: HashMap<String, (Entity<InputState>, Subscription)>,
    confirm_delete: Option<String>,
}

pub(super) fn stale(fields: &Fields, bench: &Workbench) -> bool {
    fields.snapshot != ProfileSnapshot::of(bench)
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let profiles = bench.browser_profiles();
    let selected = if profiles.len() > 1 {
        profiles.get(1).map(|p| p.id.clone())
    } else {
        profiles.first().map(|p| p.id.clone())
    };
    let mut fields = Fields {
        snapshot: ProfileSnapshot::of(bench),
        selected,
        name_inputs: HashMap::new(),
        desc_inputs: HashMap::new(),
        confirm_delete: None,
    };
    sync_inputs(&mut fields, &profiles, window, cx);
    super::change_pages(bench, cx, |pages, _| pages.browser_profiles = Some(fields));
}

pub(super) fn rebuild_choices(
    bench: &mut Workbench,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let profiles = bench.browser_profiles();
    let snapshot = ProfileSnapshot::of(bench);
    let existing_keys: Vec<String> = super::state(bench, cx)
        .and_then(|pages| pages.browser_profiles.as_ref())
        .map(|fields| fields.desc_inputs.keys().cloned().collect())
        .unwrap_or_default();
    let mut new_name_inputs = HashMap::new();
    let mut new_desc_inputs = HashMap::new();
    for profile in &profiles {
        if !existing_keys.contains(&profile.id) {
            new_name_inputs.insert(profile.id.clone(), make_name_input(profile, window, cx));
            new_desc_inputs.insert(profile.id.clone(), make_desc_input(profile, window, cx));
        }
    }
    change_fields(bench, cx, |fields, _| {
        fields.snapshot = snapshot;
        fields
            .name_inputs
            .retain(|id, _| profiles.iter().any(|p| &p.id == id));
        fields
            .desc_inputs
            .retain(|id, _| profiles.iter().any(|p| &p.id == id));
        for (id, pair) in new_name_inputs {
            fields.name_inputs.insert(id, pair);
        }
        for (id, pair) in new_desc_inputs {
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

pub(super) fn select_profile(
    bench: &mut Workbench,
    profile_id: String,
    cx: &mut Context<Workbench>,
) {
    change_fields(bench, cx, |fields, _| {
        fields.selected = Some(profile_id);
    });
}

fn sync_inputs(
    fields: &mut Fields,
    profiles: &[BrowserProfile],
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    for profile in profiles {
        if !fields.name_inputs.contains_key(&profile.id) {
            fields
                .name_inputs
                .insert(profile.id.clone(), make_name_input(profile, window, cx));
        }
        if !fields.desc_inputs.contains_key(&profile.id) {
            fields
                .desc_inputs
                .insert(profile.id.clone(), make_desc_input(profile, window, cx));
        }
    }
}

fn make_name_input(
    profile: &BrowserProfile,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t!("Profile name"))
            .default_value(profile.name.clone())
    });
    let profile_id = profile.id.clone();
    let sub = cx.subscribe_in(&input, window, move |bench, field, event, window, cx| {
        if matches!(event, InputEvent::Blur | InputEvent::PressEnter { .. }) {
            let typed = field.read(cx).value().trim().to_owned();
            let Some(current) = bench.browser_profile_by_id(&profile_id) else {
                return;
            };
            if typed.is_empty() {
                let restore = current.name.clone();
                field.update(cx, |input, cx| input.set_value(restore, window, cx));
                return;
            }
            if current.name == typed {
                return;
            }
            if bench.has_browser_profile_named(&typed, Some(&current.id)) {
                let restore = current.name.clone();
                field.update(cx, |input, cx| input.set_value(restore, window, cx));
                bench.notify(
                    toast::error(t!("A profile with this name already exists")),
                    cx,
                );
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
    (input, sub)
}

fn make_desc_input(
    profile: &BrowserProfile,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) -> (Entity<InputState>, Subscription) {
    let input = cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(t!("What it's for, e.g. Jira, staging"))
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
                Some(t!("Profile note saved").to_owned()),
                None,
                cx,
            );
        }
    });
    (input, sub)
}

fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.browser_profiles.as_mut(), change)
}

pub(super) fn header_action(_bench: &Workbench, this: &Entity<Workbench>, _cx: &App) -> AnyElement {
    let new_this = this.clone();
    Button::new("bp-new-profile", t!("New profile"), Variant::Outline)
        .icon(Icon::Plus)
        .on_click(move |_, window, cx| {
            new_this.update(cx, |bench, cx| {
                bench.open_new_browser_profile_modal(NewProfileTarget::Settings, None, window, cx);
            });
        })
        .into_any_element()
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
        bits.push(format!("{n} Playbook{}", if n == 1 { "" } else { "s" }));
    }
    if !usage.tabs.is_empty() {
        let n = usage.tabs.len();
        bits.push(format!("{n} tab{}", if n == 1 { "" } else { "s" }));
    }
    if bits.is_empty() {
        if is_global {
            t!("Every tab nothing else picks").to_owned()
        } else {
            t!("Not used yet").to_owned()
        }
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
    let multi = profiles.len() > 1;

    let global_picker = profile_picker_trigger(
        "bp-global-default-picker",
        bench,
        this.downgrade(),
        PickerMode::GlobalDefault {
            current_global_id: global.id.clone(),
        },
        Anchor::TopRight,
        false,
    );

    let default_section = rows::section(
        t!("Default"),
        None,
        None,
        lit("Default"),
        rows::row(
            t!("Global default"),
            Some(t!("When nothing else picks one")),
            global_picker,
            cx,
        ),
        cx,
    );

    let profile_rows: Vec<AnyElement> = profiles
        .iter()
        .enumerate()
        .map(|(index, profile)| {
            let pid = profile.id.clone();
            let is_global = profile.id == global.id;
            let show_default_tag =
                is_global && (multi || !profile.name.eq_ignore_ascii_case("default"));
            let is_selected = fields.selected.as_deref() == Some(profile.id.as_str());
            let usage = profile_usage(bench, &profile.id, cx);
            let summary = {
                let used = usage_summary(&usage, is_global);
                if profile.access != ProfileAccess::Standard {
                    format!("{} · {used}", access_label(profile.access))
                } else {
                    used
                }
            };
            let row_id = SharedString::from(format!("bp-row-{}", profile.id));
            let (sel_this, sel_pid) = (this.clone(), pid.clone());

            let chevron =
                ui::icon(Icon::Chevron, 14.)
                    .text_color(t.icon_quiet)
                    .when(is_selected, |chev| {
                        chev.with_transformation(Transformation::rotate(Radians(
                            std::f32::consts::FRAC_PI_2,
                        )))
                    });

            let header = div()
                .id(row_id)
                .h(px(40.))
                .px(SPACE_4)
                .flex()
                .items_center()
                .gap(px(10.))
                .when(index > 0, |row| {
                    row.border_t(theme::border()).border_color(t.line_soft)
                })
                .cursor_pointer()
                .when(is_selected, |row| row.bg(t.surface_chosen))
                .when(!is_selected, |row| {
                    row.hover(move |row| row.bg(t.surface_hover))
                })
                .clicks(move |_, _, cx| {
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
                .child(profile_mark(&profile.name, 20., TEXT_3XS_H, cx))
                .child(
                    div()
                        .flex_none()
                        .text_size(theme::ui_text(TEXT_SM))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text_primary)
                        .child(profile.name.clone()),
                )
                .when(show_default_tag, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(radius_1())
                            .border(theme::border())
                            .border_color(t.border)
                            .text_size(theme::ui_text(TEXT_3XS_H))
                            .text_color(t.text_tertiary)
                            .child(t!("Default")),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(profile.description.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(summary),
                )
                .child(chevron);

            if !is_selected {
                return header.into_any_element();
            }

            let name_field = fields
                .name_inputs
                .get(&profile.id)
                .map(|(input, _)| flat_input(input, 30., TEXT_SM, cx).into_any_element());
            let desc_field = fields
                .desc_inputs
                .get(&profile.id)
                .map(|(input, _)| flat_input(input, 30., TEXT_SM, cx).into_any_element());

            let acc_this = this.clone();
            let acc_profile = profile.clone();
            let access_control = div()
                .flex()
                .flex_col()
                .gap(SPACE_1H)
                .max_w(px(380.))
                .child(access_segmented(
                    SharedString::from(format!("bp-access-{}", profile.id)),
                    profile.access,
                    move |policy, _, cx| {
                        let current = acc_profile.clone();
                        let label = access_label(policy);
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
                    },
                    cx,
                ))
                .child(
                    div()
                        .text_size(theme::ui_text(TEXT_2XS_H))
                        .text_color(t.text_tertiary)
                        .child(access_description(profile.access)),
                );

            let mut used_rows: Vec<(Icon, String, String)> = Vec::new();
            for (space_name, space_icon) in &usage.spaces {
                let glyph = Icon::named(space_icon).unwrap_or(Icon::Folder);
                used_rows.push((
                    glyph,
                    format!("Space {space_name}"),
                    t!("default").to_owned(),
                ));
            }
            for project_name in &usage.pins {
                used_rows.push((Icon::Pin, project_name.clone(), t!("Playbook").to_owned()));
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
                used_rows.push((glyph, tab_title.clone(), tag));
            }

            let used_count = used_rows.len();
            let used_body: AnyElement =
                if used_rows.is_empty() {
                    div()
                        .pt(px(5.))
                        .text_size(theme::ui_text(TEXT_XS))
                        .text_color(t.text_tertiary)
                        .child(if is_global {
                            t!("Every tab nothing else picks.")
                        } else {
                            t!("Not used yet. Pick it from a tab’s profile pill.")
                        })
                        .into_any_element()
                } else {
                    div()
                        .flex()
                        .flex_col()
                        .children(used_rows.into_iter().enumerate().map(
                            |(at, (glyph, label, tag))| {
                                div()
                                    .h(px(30.))
                                    .flex()
                                    .items_center()
                                    .gap(SPACE_2)
                                    .when(at + 1 < used_count, |row| {
                                        row.border_b(theme::border()).border_color(t.line_soft)
                                    })
                                    .text_size(theme::ui_text(TEXT_XS_H))
                                    .text_color(t.text_secondary)
                                    .child(ui::icon(glyph, 13.).text_color(t.icon_quiet))
                                    .child(div().flex_1().min_w_0().truncate().child(label))
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(theme::ui_text(TEXT_XS))
                                            .text_color(t.text_tertiary)
                                            .child(tag),
                                    )
                            },
                        ))
                        .into_any_element()
                };

            let (def_this, def_pid) = (this.clone(), profile.id.clone());
            let (clear_this, clear_pid) = (this.clone(), profile.id.clone());
            let (del_btn_this, del_btn_pid) = (this.clone(), profile.id.clone());
            let danger_ink = t.danger;
            let danger_hover = t.danger.opacity(0.08);

            let detail_row = |label: &'static str,
                              sub: Option<&'static str>,
                              control: AnyElement,
                              align_start: bool| {
                div()
                    .flex()
                    .when(align_start, |r| r.items_start())
                    .when(!align_start, |r| r.items_center())
                    .gap(SPACE_4)
                    .child(
                        div()
                            .w(px(150.))
                            .flex_none()
                            .when(align_start, |col| col.pt(px(5.)))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(theme::ui_text(TEXT_XS_H))
                                    .text_color(t.text_secondary)
                                    .child(label),
                            )
                            .when_some(sub, |col, sub| {
                                col.child(
                                    div()
                                        .text_size(theme::ui_text(TEXT_3XS_H))
                                        .text_color(t.text_tertiary)
                                        .child(sub),
                                )
                            }),
                    )
                    .child(div().flex_1().min_w_0().child(control))
            };

            let detail = div()
                .pl(px(46.))
                .pr(SPACE_4)
                .pt(px(14.))
                .pb(px(18.))
                .flex()
                .flex_col()
                .gap(px(14.))
                .when_some(name_field, |col, field| {
                    col.child(detail_row(t!("Name"), None, field, false))
                })
                .when_some(desc_field, |col, field| {
                    col.child(detail_row(
                        t!("Note"),
                        Some(t!("What agents read when choosing a profile")),
                        field,
                        false,
                    ))
                })
                .child(detail_row(
                    t!("Agent access"),
                    None,
                    access_control.into_any_element(),
                    true,
                ))
                .child(detail_row(t!("Used by"), None, used_body, true))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(SPACE_1H)
                        .ml(px(-10.))
                        .when(!is_global, |bar| {
                            bar.child(
                                Button::new(
                                    SharedString::from(format!("bp-set-default-{}", profile.id)),
                                    t!("Set as default"),
                                    Variant::Ghost,
                                )
                                .on_click(move |_, _, cx| {
                                    let pid = def_pid.clone();
                                    def_this.update(cx, |bench, cx| {
                                        bench.set_global_browser_profile(pid, cx);
                                    });
                                }),
                            )
                        })
                        .child(
                            Button::new(
                                SharedString::from(format!("bp-clear-{}", profile.id)),
                                t!("Clear site data"),
                                Variant::Ghost,
                            )
                            .icon(Icon::Discard)
                            .on_click(move |_, _, cx| {
                                let pid = clear_pid.clone();
                                clear_this.update(cx, |bench, cx| {
                                    bench.clear_browser_profile_data(&pid, cx);
                                });
                            }),
                        )
                        .when(!profile.is_default_store, |bar| {
                            bar.child(
                                div()
                                    .id(SharedString::from(format!("bp-del-detail-{}", profile.id)))
                                    .h(px(28.))
                                    .px(px(11.))
                                    .rounded(radius_2())
                                    .flex()
                                    .items_center()
                                    .gap(SPACE_1H)
                                    .cursor_pointer()
                                    .text_size(theme::ui_text(TEXT_XS_H))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(danger_ink)
                                    .hover(move |btn| btn.bg(danger_hover))
                                    .clicks(move |_, _, cx| {
                                        let pid = del_btn_pid.clone();
                                        del_btn_this.update(cx, |bench, cx| {
                                            change_fields(bench, cx, |fields, _| {
                                                fields.confirm_delete = Some(pid);
                                            });
                                        });
                                    })
                                    .child(ui::icon(Icon::Trash, 13.).text_color(danger_ink))
                                    .child(t!("Delete profile")),
                            )
                        })
                        .when(profile.is_default_store, |bar| {
                            bar.child(
                                div()
                                    .pl(SPACE_2)
                                    .text_size(theme::ui_text(TEXT_2XS_H))
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

    let invite_this = this.clone();
    let single_profile_invite = (!multi).then(|| {
        div()
            .id("bp-empty-invite")
            .px(SPACE_4)
            .py(SPACE_3)
            .border_t(theme::border())
            .border_color(t.line_soft)
            .flex()
            .items_center()
            .gap(SPACE_2)
            .cursor_pointer()
            .text_size(theme::ui_text(TEXT_XS_H))
            .text_color(t.text_tertiary)
            .hover(move |row| row.text_color(t.text_primary))
            .clicks(move |_, window, cx| {
                invite_this.update(cx, |bench, cx| {
                    bench.open_new_browser_profile_modal(
                        NewProfileTarget::Settings,
                        None,
                        window,
                        cx,
                    );
                });
            })
            .child(ui::icon(Icon::Plus, 13.).text_color(t.icon_quiet))
            .child(t!(
                "Add a second profile to keep work and personal sign-ins apart."
            ))
    });

    let profiles_section = rows::section(
        t!("Profiles"),
        multi.then(|| format!("{}", profiles.len()).into()),
        None,
        lit("Profiles"),
        div()
            .flex()
            .flex_col()
            .children(profile_rows)
            .children(single_profile_invite),
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

    let close_this = this.clone();
    let confirm_this = this.clone();
    let pid = profile.id.clone();

    let no: dialog::Answer = Rc::new(move |_, cx| {
        close_this.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| fields.confirm_delete = None);
        });
    });
    let yes: dialog::Answer = Rc::new(move |_, cx| {
        let pid = pid.clone();
        confirm_this.update(cx, |bench, cx| {
            change_fields(bench, cx, |fields, _| fields.confirm_delete = None);
            bench.delete_browser_profile(pid, cx);
        });
    });

    let close_x = no.clone();
    let cancel_act = no.clone();
    let delete_act = yes.clone();

    let content = div()
        .flex()
        .flex_col()
        .child(
            div()
                .pl(px(18.))
                .pr(px(14.))
                .pt(px(14.))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    ui::text(
                        format!("Delete “{}”?", profile.name),
                        TEXT_MD,
                        FontWeight::SEMIBOLD,
                    )
                    .text_color(t.text_primary),
                )
                .child(
                    IconButton::new("bp-del-close", Icon::Close, t!("Close"))
                        .compact()
                        .on_click(move |_, window, cx| close_x(window, cx)),
                ),
        )
        .child(
            div()
                .px(px(18.))
                .pt(px(12.))
                .pb(px(16.))
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
                                .child(ui::icon(icon, 13.).text_color(t.icon_quiet)),
                        )
                        .child(div().flex_1().min_w_0().child(text))
                })),
        )
        .child(
            div()
                .px(px(14.))
                .py(px(12.))
                .border_t(theme::border())
                .border_color(t.line_soft)
                .flex()
                .items_center()
                .justify_end()
                .gap(SPACE_2)
                .child(
                    Button::new("bp-del-cancel", t!("Cancel"), Variant::Ghost)
                        .on_click(move |_, window, cx| cancel_act(window, cx)),
                )
                .child(
                    Button::new("bp-del-confirm", t!("Delete profile"), Variant::Primary)
                        .solid(t.danger, t.danger.opacity(0.88))
                        .on_click(move |_, window, cx| delete_act(window, cx)),
                ),
        );

    Some(
        dialog::floating(px(400.), content, yes, no)
            .flat()
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use gpui::{size, Bounds, Modifiers, Pixels, Render, TestAppContext, VisualTestContext};
    use serde_json::json;
    use sikemux_calls::browser_profiles::{
        BrowserProfileSetSpaceAnswer, BrowserProfileUpsertAnswer,
    };
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
            let settings = self
                .0
                .update(cx, |bench, cx| crate::settings::view(bench, window, cx));
            let dialogs = self
                .0
                .update(cx, |bench, cx| crate::overlays::dialogs(bench, cx));
            div()
                .relative()
                .size_full()
                .child(settings)
                .children(dialogs)
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
    fn clicking_new_profile_opens_modal_and_creates_without_panicking(cx: &mut TestAppContext) {
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
                name: "Work".into(),
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
        assert_eq!(new_btn.len(), 1, "New profile button is visible in header");
        window.simulate_click(new_btn[0].center(), Modifiers::none());
        settle(window);

        window.update(|window, cx| {
            let name_input = {
                let modal = bench
                    .read(cx)
                    .overlays
                    .adding_browser_profile
                    .as_ref()
                    .expect("Modal M1 opened");
                assert!(!modal.agent_access_open, "Agent access starts folded");
                modal.name_input.clone()
            };
            name_input.update(cx, |input, cx| {
                input.set_value("Client staging", window, cx);
            });
        });
        settle(window);

        let create_btn = drawn(window, "bp-modal-create");
        assert_eq!(create_btn.len(), 1, "Create CTA is visible");
        window.simulate_click(create_btn[0].center(), Modifiers::none());
        settle(window);

        let mut created = None;
        while let Ok(ask) = asked.try_recv() {
            let Ask::Request(method, reply) = ask else {
                continue;
            };
            if let Some(params) = method.params_of::<call::BrowserProfileUpsert>() {
                assert_eq!(params.name, "Client staging");
                let profile = BrowserProfile {
                    id: "profile-3".into(),
                    slug: "client-staging".into(),
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
            assert!(
                bench.overlays.adding_browser_profile.is_none(),
                "Modal closes after creation"
            );
            let fields = super::super::state(bench, cx)
                .and_then(|pages| pages.browser_profiles.as_ref())
                .expect("browser profiles page initialized");
            assert_eq!(fields.selected.as_deref(), Some("profile-3"));
        });

        send_prefs(&profiles);
        settle(window);

        window.update(|_, cx| {
            let bench = bench.read(cx);
            let fields = super::super::state(bench, cx)
                .and_then(|pages| pages.browser_profiles.as_ref())
                .expect("browser profiles page initialized");
            assert_eq!(fields.selected.as_deref(), Some("profile-3"));
        });
    }

    #[gpui::test]
    fn modal_space_prefill_submits_and_assigns_without_duplicating(cx: &mut TestAppContext) {
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
        let mut profiles = vec![default_profile()];
        let mut bench_handle = None;
        let (_, window) = cx.add_window_view(|window, cx| {
            let bench = cx.new(|cx| Workbench::new(link, window, cx));
            bench_handle = Some(bench.clone());
            let shown = cx.new(|_| Shown(bench));
            gpui_kit::base::Root::new(shown, window, cx)
        });
        let bench = bench_handle.expect("workbench created");
        window.simulate_resize(size(px(1200.), px(900.)));

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

        // Open Modal M1 for a Space with initial_name prefilled.
        window.update(|window, cx| {
            bench.update(cx, |bench, cx| {
                bench.open_new_browser_profile_modal(
                    NewProfileTarget::Space {
                        space_id: "space-work".into(),
                        space_name: "Work".into(),
                    },
                    Some("Work".into()),
                    window,
                    cx,
                );
            });
        });
        settle(window);

        // Submitting immediately uses prefilled "Work" and assigns it to the Space.
        let create_btn = drawn(window, "bp-modal-create");
        assert_eq!(create_btn.len(), 1);
        window.simulate_click(create_btn[0].center(), Modifiers::none());
        settle(window);

        let mut assigned_space = false;
        while let Ok(ask) = asked.try_recv() {
            let Ask::Request(method, reply) = ask else {
                continue;
            };
            if let Some(params) = method.params_of::<call::BrowserProfileUpsert>() {
                assert_eq!(params.name, "Work");
                let profile = BrowserProfile {
                    id: "profile-work".into(),
                    slug: "work".into(),
                    name: params.name,
                    description: String::new(),
                    access: ProfileAccess::Standard,
                    is_default_store: false,
                    created_at: 2,
                };
                profiles.push(profile.clone());
                let answer = BrowserProfileUpsertAnswer {
                    profile,
                    profiles: profiles.clone(),
                    global_default_id: DEFAULT_PROFILE_ID.into(),
                };
                let _ = reply.send(Ok(json!(answer)));
            }
        }
        settle(window);
        while let Ok(ask) = asked.try_recv() {
            let Ask::Request(method, reply) = ask else {
                continue;
            };
            if let Some(params) = method.params_of::<call::BrowserProfileSetSpace>() {
                assert_eq!(params.space_id, "space-work");
                assert_eq!(params.profile_id.as_deref(), Some("profile-work"));
                assigned_space = true;
                let _ = reply.send(Ok(json!(BrowserProfileSetSpaceAnswer {
                    space_id: "space-work".into(),
                    profile: profiles.last().cloned(),
                    created: false,
                })));
            }
        }
        assert!(assigned_space, "newly created profile assigned to Space");

        // Duplicate name is blocked when opening Modal M1 again.
        send_prefs(&profiles);
        settle(window);
        window.update(|window, cx| {
            assert!(
                bench.read(cx).has_browser_profile_named("work", None),
                "Work profile is registered in preferences"
            );
            bench.update(cx, |bench, cx| {
                bench.open_new_browser_profile_modal(
                    NewProfileTarget::Settings,
                    Some("work".into()),
                    window,
                    cx,
                );
            });
        });
        settle(window);
        while asked.try_recv().is_ok() {}
        let create_btn = drawn(window, "bp-modal-create");
        window.simulate_click(create_btn[0].center(), Modifiers::none());
        settle(window);
        let mut duplicate_upsert_sent = false;
        while let Ok(ask) = asked.try_recv() {
            if let Ask::Request(method, _) = ask {
                if method.params_of::<call::BrowserProfileUpsert>().is_some() {
                    duplicate_upsert_sent = true;
                }
            }
        }
        assert!(
            !duplicate_upsert_sent,
            "duplicate profile name must not send upsert"
        );
    }
}
