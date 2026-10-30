//! Shell commands the person adds beside the built-in ones.

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
};
use sikemux_wire::protocol::{CustomCommand, CustomCommandPlacement};

use crate::icons::Icon;
use crate::settings::rows;
use crate::settings::Settings;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_2, SPACE_2H, SPACE_3H, SPACE_4, TEXT_2XS_H, TEXT_SM, TEXT_XS,
};
use crate::ui;
use crate::workbench::Workbench;

/// The kinds of tab an action can be offered in.
const CONTEXTS: [&str; 3] = ["project", "command", "ssh"];

const PLACEMENTS: [(CustomCommandPlacement, Icon, &str); 5] = [
    (CustomCommandPlacement::Terminal, Icon::Terminal, "Terminal"),
    (CustomCommandPlacement::Split, Icon::PanelRight, "Split"),
    (CustomCommandPlacement::Popup, Icon::Window, "Popup"),
    (CustomCommandPlacement::Background, Icon::Bolt, "Background"),
    (CustomCommandPlacement::Replace, Icon::Rotate, "Replace"),
];

/// The action being written: a saved one being changed, or a new one.
pub struct Fields {
    id: String,
    title: Entity<InputState>,
    detail: Entity<InputState>,
    command: Entity<TextareaState>,
    placement: CustomCommandPlacement,
    contexts: Vec<String>,
    _typing: Vec<Subscription>,
}

fn new_id() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("command-{now:x}")
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let title = cx.new(|cx| InputState::new(window, cx).placeholder("Display name"));
    let detail = cx.new(|cx| InputState::new(window, cx).placeholder("What it does"));
    let command = cx.new(|cx| TextareaState::new(window, cx).placeholder("shell command"));
    let typing = vec![
        super::follow_typing(bench, &title, cx),
        super::follow_typing(bench, &detail, cx),
        super::follow_typing(bench, &command, cx),
    ];
    let fields = Fields {
        id: new_id(),
        title,
        detail,
        command,
        placement: CustomCommandPlacement::Terminal,
        contexts: Vec::new(),
        _typing: typing,
    };
    super::change_pages(bench, cx, |pages, _| pages.actions = Some(fields));
}

/// Changes the action being written, and draws the pages again.
fn change_fields<R>(
    bench: &Workbench,
    cx: &mut App,
    change: impl FnOnce(&mut Fields, &mut Context<Settings>) -> R,
) -> Option<R> {
    super::change_form(bench, cx, |pages| pages.actions.as_mut(), change)
}

/// Puts `command` in the form, or empties it for a new one.
fn load(
    bench: &mut Workbench,
    command: Option<CustomCommand>,
    window: &mut Window,
    cx: &mut Context<Workbench>,
) {
    let command = command.unwrap_or_else(|| CustomCommand {
        id: new_id(),
        title: String::new(),
        detail: String::new(),
        command: String::new(),
        contexts: Vec::new(),
        placement: CustomCommandPlacement::Terminal,
    });
    change_fields(bench, cx, |fields, cx| {
        fields
            .title
            .update(cx, |input, cx| input.set_value(command.title, window, cx));
        fields
            .detail
            .update(cx, |input, cx| input.set_value(command.detail, window, cx));
        fields
            .command
            .update(cx, |input, cx| input.set_value(command.command, window, cx));
        fields.id = command.id;
        fields.placement = command.placement;
        fields.contexts = command.contexts;
    });
}

/// Changes the person's actions in the core.
fn change(bench: &Workbench, edit: impl FnOnce(&mut Vec<CustomCommand>)) {
    let mut commands = bench.preferences().custom_commands.clone();
    edit(&mut commands);
    crate::settings::set(
        bench,
        "customCommands",
        serde_json::to_value(commands).unwrap_or_default(),
    );
}

fn save(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(fields) = super::state(bench, cx).and_then(|pages| pages.actions.as_ref()) else {
        return;
    };
    let command = CustomCommand {
        id: fields.id.clone(),
        title: fields.title.read(cx).value().trim().to_owned(),
        detail: fields.detail.read(cx).value().trim().to_owned(),
        command: fields.command.read(cx).value().to_string(),
        contexts: fields.contexts.clone(),
        placement: fields.placement,
    };
    if command.title.is_empty() || command.command.trim().is_empty() {
        return;
    }
    change(bench, |commands| {
        commands.retain(|saved| saved.id != command.id);
        commands.push(command);
    });
    load(bench, None, window, cx);
}

fn placement_name(placement: CustomCommandPlacement) -> &'static str {
    PLACEMENTS
        .iter()
        .find(|(each, _, _)| *each == placement)
        .map_or("", |(_, _, name)| *name)
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.actions else {
        return Vec::new();
    };
    let t = Tokens::get(cx).clone();
    let commands = bench.preferences().custom_commands.clone();
    let editing = commands.iter().any(|command| command.id == fields.id);

    let saved: AnyElement = match commands.is_empty() {
        true => rows::note(
            "No custom actions yet. The built-ins are already searchable from the command deck.",
            cx,
        ),
        false => div()
            .children(commands.iter().map(|command| {
                let this = this.clone();
                let chosen = command.id == fields.id;
                let picked = command.clone();
                let contexts = match command.contexts.is_empty() {
                    true => "all contexts".to_owned(),
                    false => command.contexts.join(", "),
                };
                div()
                    .id(SharedString::from(format!("action-{}", command.id)))
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .px(SPACE_4)
                    .py(px(9.))
                    .border_b_1()
                    .border_color(t.line_soft)
                    .cursor_pointer()
                    .when_else(
                        chosen,
                        |row| row.bg(t.surface_chosen),
                        |row| row.hover(|style| style.bg(t.surface_hover)),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_SM))
                            .text_color(t.text_primary)
                            .child(command.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .text_color(t.text_tertiary)
                            .child(format!(
                                "{} · {contexts}",
                                placement_name(command.placement).to_lowercase()
                            )),
                    )
                    .on_click(move |_, window, cx| {
                        let picked = picked.clone();
                        this.update(cx, |bench, cx| load(bench, Some(picked), window, cx))
                    })
                    .into_any_element()
            }))
            .into_any_element(),
    };

    let placement = {
        let this = this.clone();
        let chosen = PLACEMENTS
            .iter()
            .position(|(each, _, _)| *each == fields.placement)
            .unwrap_or(0);
        ui::Segmented::new(
            "action-placement",
            PLACEMENTS
                .iter()
                .map(|(_, icon, name)| (*icon, SharedString::from(*name)))
                .collect(),
            chosen,
        )
        .on_select(move |chosen, _, cx| {
            let placement = PLACEMENTS[*chosen].0;
            this.update(cx, |bench, cx| {
                change_fields(bench, cx, |fields, _| fields.placement = placement)
            });
        })
    };
    let contexts = div()
        .flex()
        .flex_wrap()
        .gap(SPACE_3H)
        .children(CONTEXTS.iter().map(|context| {
            let this = this.clone();
            Checkbox::new(SharedString::from(format!("action-context-{context}")))
                .label(*context)
                .checked(fields.contexts.iter().any(|chosen| chosen == context))
                .on_click(move |on: &bool, _, cx| {
                    let on = *on;
                    this.update(cx, |bench, cx| {
                        change_fields(bench, cx, |fields, _| {
                            fields.contexts.retain(|chosen| chosen != context);
                            if on {
                                fields.contexts.push((*context).to_owned());
                            }
                        })
                    });
                })
        }));
    let stacked = |label: &'static str, about: &'static str, control: AnyElement| {
        div()
            .flex()
            .flex_col()
            .gap(SPACE_2)
            .px(SPACE_4)
            .py(SPACE_2H)
            .border_b_1()
            .border_color(t.line_soft)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(SPACE_0)
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_SM))
                            .text_color(t.text_primary)
                            .child(label),
                    )
                    .child(
                        div()
                            .text_size(crate::theme::ui_text(TEXT_2XS_H))
                            .text_color(t.text_tertiary)
                            .child(about),
                    ),
            )
            .child(control)
    };
    let ready = !fields.title.read(cx).value().trim().is_empty()
        && !fields.command.read(cx).value().trim().is_empty();
    let (fresh, delete, keep) = (this.clone(), this.clone(), this.clone());
    let id = fields.id.clone();
    let buttons = div()
        .flex()
        .justify_end()
        .gap(SPACE_2)
        .p(SPACE_4)
        .child(
            ui::Button::new("action-new", "New", ui::Variant::Outline).on_click(
                move |_, window, cx| fresh.update(cx, |bench, cx| load(bench, None, window, cx)),
            ),
        )
        .when(editing, |buttons| {
            buttons.child(
                ui::Button::new("action-delete", "Delete", ui::Variant::Outline)
                    .icon(Icon::Trash)
                    .on_click(move |_, window, cx| {
                        let id = id.clone();
                        delete.update(cx, |bench, cx| {
                            change(bench, |commands| commands.retain(|saved| saved.id != id));
                            load(bench, None, window, cx);
                        })
                    }),
            )
        })
        .child(
            ui::Button::new("action-save", "Save", ui::Variant::Primary)
                .icon(Icon::Check)
                .disabled(!ready)
                .on_click(move |_, window, cx| {
                    keep.update(cx, |bench, cx| save(bench, window, cx))
                }),
        );
    let form = div()
        .child(rows::row(
            "Name",
            None,
            div().w(px(320.)).child(ui::Field::new(&fields.title)),
            cx,
        ))
        .child(rows::row(
            "Description",
            Some("Shown under the name in the deck."),
            div().w(px(320.)).child(ui::Field::new(&fields.detail)),
            cx,
        ))
        .child(stacked(
            "Command",
            "Runs unsandboxed in the active session's directory, with SIKEMUX_SESSION_* and SIKEMUX_PROJECT set.",
            div()
                .border_1()
                .border_color(t.line)
                .rounded(theme::RADIUS_2)
                .font_family(theme::MONO)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .child(Textarea::new(&fields.command).h(px(96.)).bordered(false))
                .into_any_element(),
        ))
        .child(stacked(
            "Where output lands",
            "A terminal tab, a split, a popup, a background toast, or this pane.",
            div().child(placement).into_any_element(),
        ))
        .child(stacked(
            "Contexts",
            "Leave all unticked to offer it everywhere.",
            contexts.into_any_element(),
        ))
        .child(buttons);

    vec![
        rows::section(
            "Your actions",
            Some(format!("{} saved", commands.len()).into()),
            Some("Shell commands that sit beside the built-in ones.".into()),
            lit("Your actions"),
            saved,
            cx,
        ),
        rows::section(
            if editing { "Edit action" } else { "New action" },
            None,
            None,
            lit("New action"),
            form,
            cx,
        ),
    ]
}
