//! Which program and account folder each launch uses. Profiles never hold credentials.

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
};
use sikemux_core::agent_accounts::Profile;

use crate::icons::{self, Icon};
use crate::settings::rows;
use crate::settings::Settings;
use crate::theme::{
    self, Tokens, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4, TEXT_2XS, TEXT_XS_H,
};
use crate::ui;
use crate::workbench::Workbench;

/// The profile being written: a saved one being changed, or a new one.
pub struct Fields {
    id: String,
    provider: String,
    name: Entity<InputState>,
    program: Entity<InputState>,
    folder: Entity<InputState>,
    _typing: Subscription,
}

fn new_id() -> String {
    format!("profile-{:x}", chrono::Utc::now().timestamp_millis())
}

impl Fields {
    pub fn new(bench: &Workbench, window: &mut Window, cx: &mut Context<Workbench>) -> Self {
        let name = cx.new(|cx| InputState::new(window, cx));
        let typing = super::super::follow_typing(bench, &name, cx);
        Self {
            id: new_id(),
            provider: "claude".into(),
            name,
            _typing: typing,
            program: cx
                .new(|cx| InputState::new(window, cx).placeholder("Leave empty to use PATH")),
            folder: cx
                .new(|cx| InputState::new(window, cx).placeholder("Automatic, or ~/.claude-work")),
        }
    }
}

/// Changes the profile being written, and draws the pages again.
fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_fields(bench, cx, |fields, cx| change(&mut fields.profiles, cx))
}

/// The agents that keep each sign-in in a folder of their own.
fn providers(bench: &Workbench) -> Vec<(String, String)> {
    bench
        .reads
        .catalog
        .agents()
        .filter(|kind| kind.accounts)
        .map(|kind| (kind.kind.clone(), kind.label.clone()))
        .collect()
}

/// Puts `profile` in the editor, or empties it for a new one.
fn load(
    bench: &mut Workbench,
    profile: Option<Profile>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let profile = profile.unwrap_or_else(|| Profile {
        id: new_id(),
        name: String::new(),
        provider: "claude".into(),
        accent: String::new(),
        executable_path: None,
        config_path: None,
        environment_keys: Vec::new(),
    });
    change_fields(bench, cx, |fields, cx| {
        let mut set = |input: &Entity<InputState>, value: String, cx: &mut Context<Settings>| {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        };
        set(&fields.name, profile.name, cx);
        set(
            &fields.program,
            profile.executable_path.unwrap_or_default(),
            cx,
        );
        set(&fields.folder, profile.config_path.unwrap_or_default(), cx);
        fields.id = profile.id;
        fields.provider = profile.provider;
    });
}

fn save(bench: &mut Workbench, cx: &mut Context<Workbench>) {
    let Some(fields) = super::fields(bench, cx).map(|agents| &agents.profiles) else {
        return;
    };
    let text = |input: &Entity<InputState>| {
        Some(input.read(cx).value().trim().to_owned()).filter(|text| !text.is_empty())
    };
    let Some(name) = text(&fields.name) else {
        return;
    };
    let saved = bench.accounts.profile(&fields.id);
    let profile = Profile {
        id: fields.id.clone(),
        name,
        provider: fields.provider.clone(),
        accent: saved
            .map(|saved| saved.accent.clone())
            .filter(|_| saved.is_some_and(|saved| saved.provider == fields.provider))
            .or_else(|| {
                bench
                    .reads
                    .catalog
                    .kind(&fields.provider)
                    .map(|kind| kind.brand.clone())
            })
            .unwrap_or_default(),
        executable_path: text(&fields.program),
        config_path: text(&fields.folder),
        environment_keys: saved
            .map(|saved| saved.environment_keys.clone())
            .unwrap_or_default(),
    };
    crate::accounts::save(bench, &profile, cx);
}

pub fn render(
    bench: &Workbench,
    fields: &Fields,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let profiles: Vec<Profile> = bench
        .accounts
        .book
        .as_ref()
        .map(|book| book.profiles.clone())
        .unwrap_or_default();
    let saved = profiles.iter().any(|profile| profile.id == fields.id);
    let built_in = fields.id.starts_with("builtin-");

    let list = div()
        .w(px(180.))
        .flex_none()
        .flex()
        .flex_col()
        .border_r_1()
        .border_color(t.line_soft)
        .children(profiles.iter().map(|profile| {
            let chosen = profile.id == fields.id;
            let (this, picked) = (this.clone(), profile.clone());
            div()
                .id(SharedString::from(format!("profile-{}", profile.id)))
                .flex()
                .items_center()
                .gap(SPACE_2)
                .px(SPACE_3)
                .py(SPACE_2)
                .cursor_pointer()
                .when(chosen, |row| row.bg(t.surface_chosen))
                .when(!chosen, |row| row.hover(|style| style.bg(t.surface_hover)))
                .child(
                    div()
                        .size(px(8.))
                        .flex_none()
                        .rounded_full()
                        .bg(crate::accounts::accent(profile)),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(crate::theme::ui_text(TEXT_XS_H))
                                .text_color(t.text_primary)
                                .child(profile.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(crate::theme::ui_text(TEXT_2XS))
                                .text_color(t.text_tertiary)
                                .child(format!(
                                    "{} · {}",
                                    profile.provider,
                                    profile.executable_path.as_deref().unwrap_or("system PATH")
                                )),
                        ),
                )
                .on_click(move |_, window, cx| {
                    let picked = picked.clone();
                    this.update(cx, |bench, cx| load(bench, Some(picked), window, cx))
                })
        }))
        .child({
            let this = this.clone();
            div().p(SPACE_2).child(
                ui::Button::new("profile-new", "New profile", ui::Variant::Dashed)
                    .icon(Icon::Plus)
                    .on_click(move |_, window, cx| {
                        this.update(cx, |bench, cx| load(bench, None, window, cx))
                    }),
            )
        });

    let offered = providers(bench);
    let provider = {
        let this = this.clone();
        let kinds: Vec<String> = offered.iter().map(|(kind, _)| kind.clone()).collect();
        let chosen = kinds
            .iter()
            .position(|kind| *kind == fields.provider)
            .unwrap_or(0);
        ui::Segmented::new(
            "profile-provider",
            offered
                .iter()
                .map(|(kind, label)| (icons::agent(kind), SharedString::from(label.clone())))
                .collect(),
            chosen,
        )
        .on_select(move |at, _, cx| {
            let Some(kind) = kinds.get(*at).cloned() else {
                return;
            };
            this.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| {
                    if !fields.id.starts_with("builtin-") {
                        fields.provider = kind;
                    }
                });
            })
        })
    };
    let has_accounts = offered.iter().any(|(kind, _)| *kind == fields.provider);
    let stacked = |label: &'static str, about: Option<String>, control: AnyElement| {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_1H)
            .px(SPACE_4)
            .py(SPACE_2H)
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                div()
                    .text_size(crate::theme::ui_text(TEXT_XS_H))
                    .text_color(t.text_primary)
                    .child(label),
            )
            .children(about.map(|about| {
                div()
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_tertiary)
                    .child(about)
            }))
            .child(control)
    };
    let field = |input: &Entity<InputState>| ui::Field::new(input).into_any_element();
    let (keep, remove) = (this.clone(), this.clone());
    let id = fields.id.clone();
    let editor = div()
        .flex_1()
        .min_w_0()
        .child(stacked("Name", None, field(&fields.name)))
        .child(stacked(
            "Agent",
            None,
            div()
                .when(built_in, |row| row.opacity(0.5))
                .child(provider)
                .into_any_element(),
        ))
        .child(stacked(
            "Executable path",
            Some("Empty uses PATH.".into()),
            field(&fields.program),
        ))
        .when(has_accounts, |editor| {
            editor.child(stacked(
                "Profile directory",
                Some(format!(
                    "Set this only when keeping several {} accounts side by side.",
                    fields.provider
                )),
                field(&fields.folder),
            ))
        })
        .child(
            div()
                .flex()
                .justify_end()
                .gap(SPACE_2)
                .p(SPACE_4)
                .when(saved && !built_in, |actions| {
                    actions.child(
                        ui::Button::new("profile-delete", "Delete", ui::Variant::Outline)
                            .icon(Icon::Trash)
                            .on_click(move |_, window, cx| {
                                let id = id.clone();
                                remove.update(cx, |bench, cx| {
                                    crate::accounts::remove(bench, &id, cx);
                                    load(bench, None, window, cx);
                                })
                            }),
                    )
                })
                .child(
                    ui::Button::new(
                        "profile-save",
                        if saved { "Save profile" } else { "Add profile" },
                        ui::Variant::Primary,
                    )
                    .icon(Icon::Check)
                    .disabled(fields.name.read(cx).value().trim().is_empty())
                    .on_click(move |_, _, cx| keep.update(cx, save)),
                ),
        );
    rows::section(
        "Agent profiles",
        Some(format!("{} configured", profiles.len()).into()),
        Some("Which local executable a launch uses. Profiles never hold credentials.".into()),
        lit("Agent profiles"),
        div()
            .flex()
            .child(list)
            .child(editor)
            .rounded(theme::RADIUS_3),
        cx,
    )
}
