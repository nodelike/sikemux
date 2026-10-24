//! Every command's keys, and recording new ones.

use std::collections::BTreeMap;
use std::time::Duration;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Animation, AnimationExt, AnyElement, AppContext, Context, Entity, InteractiveElement,
    IntoElement, Keystroke, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Window,
};

use crate::icons::Icon;
use crate::keymap::{self, Shortcut};
use crate::settings::rows;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

pub const SECTIONS: &[&str] = &["Shortcuts"];

/// The filter, the command whose keys are being recorded, and what the last change did.
pub struct Fields {
    filter: Entity<InputState>,
    recording: Option<Recording>,
    message: Option<SharedString>,
    _typing: Subscription,
}

/// A command waiting for its new keys, which reach it before any shortcut does.
struct Recording {
    id: &'static str,
    _keys: Subscription,
}

fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let Some(settings) = bench.settings.as_mut() else {
        return;
    };
    let jump = settings.pages.shortcut.take().and_then(|id| {
        keymap::shortcuts()
            .iter()
            .find(|shortcut| shortcut.id == id)
            .map(|shortcut| shortcut.label)
    });
    let filter = cx.new(|cx| {
        let mut input =
            InputState::new(window, cx).placeholder("Filter by name, description or key");
        if let Some(label) = jump {
            input.set_value(label, window, cx);
        }
        input
    });
    let typing = cx.subscribe(&filter, |bench, _, event: &InputEvent, cx| {
        if matches!(event, InputEvent::Change) {
            bench.parts.redraw(&bench.parts.stage, cx);
        }
    });
    settings.pages.keybindings = Some(Fields {
        filter,
        recording: None,
        message: None,
        _typing: typing,
    });
    bench.parts.redraw(&bench.parts.stage, cx);
}

fn fields_of(bench: &mut Workbench) -> Option<&mut Fields> {
    bench.settings.as_mut()?.pages.keybindings.as_mut()
}

/// Changes the keys the person chose, in the core.
fn change(bench: &Workbench, edit: impl FnOnce(&mut BTreeMap<String, Option<String>>)) {
    let mut overrides = bench.preferences().keybinding_overrides;
    edit(&mut overrides);
    crate::settings::set(
        bench,
        "keybindingOverrides",
        serde_json::to_value(overrides).unwrap_or_default(),
    );
}

fn say(bench: &mut Workbench, message: String, cx: &mut Context<Workbench>) {
    if let Some(fields) = fields_of(bench) {
        fields.message = Some(message.into());
    }
    bench.parts.redraw(&bench.parts.stage, cx);
}

fn stop(bench: &mut Workbench, message: String, cx: &mut Context<Workbench>) {
    if let Some(fields) = fields_of(bench) {
        fields.recording = None;
    }
    say(bench, message, cx);
}

fn record(bench: &mut Workbench, id: &'static str, cx: &mut Context<Workbench>) {
    let this = cx.entity().downgrade();
    let keys = cx.intercept_keystrokes(move |event, _, cx| {
        cx.stop_propagation();
        let keystroke = event.keystroke.clone();
        let _ = this.update(cx, |bench, cx| capture(bench, &keystroke, cx));
    });
    if let Some(fields) = fields_of(bench) {
        fields.recording = Some(Recording { id, _keys: keys });
    }
    say(
        bench,
        "Press a shortcut. Backspace clears it; Escape cancels.".into(),
        cx,
    );
}

fn keys_label(keys: Option<&str>) -> SharedString {
    keys.and_then(keymap::label)
        .unwrap_or_else(|| "Unassigned".into())
}

/// Binds the recorded command to the keys pressed, unless they cannot take it.
fn capture(bench: &mut Workbench, keystroke: &Keystroke, cx: &mut Context<Workbench>) {
    let Some(id) = fields_of(bench).and_then(|fields| Some(fields.recording.as_ref()?.id)) else {
        return;
    };
    let Some(shortcut) = keymap::shortcuts()
        .iter()
        .find(|shortcut| shortcut.id == id)
    else {
        return;
    };
    match keystroke.key.as_str() {
        "escape" => return stop(bench, "Change cancelled.".into(), cx),
        "backspace" | "delete" => {
            change(bench, |overrides| {
                overrides.insert(id.to_owned(), None);
            });
            return stop(bench, format!("{} is now unassigned.", shortcut.label), cx);
        }
        _ => {}
    }
    let Some(keys) = keymap::spelling(keystroke) else {
        return;
    };
    let shown = keys_label(Some(&keys));
    if !keymap::has_modifier(&keys) {
        let which = match cfg!(target_os = "macos") {
            true => "Command, Control or Option",
            false => "Control or Alt",
        };
        return say(
            bench,
            format!("Add {which}: Shift alone only changes what a key types."),
            cx,
        );
    }
    if let Some(what) = keymap::reserved(&keys) {
        return say(
            bench,
            format!("{shown} is {what}, which stays where it is."),
            cx,
        );
    }
    let overrides = bench.preferences().keybinding_overrides;
    let taken = keymap::shortcuts()
        .iter()
        .find(|other| other.id != id && other.keys(&overrides).as_deref() == Some(keys.as_str()))
        .map(|other| other.label);
    if let Some(other) = taken {
        return say(
            bench,
            format!("{shown} is already assigned to “{other}”."),
            cx,
        );
    }
    let option_only = keys.starts_with("Alt+");
    change(bench, |overrides| {
        overrides.insert(id.to_owned(), Some(keys));
    });
    stop(
        bench,
        format!(
            "{} changed to {shown}.{}",
            shortcut.label,
            match option_only {
                true => " It stays with the terminal or text field while you type in one.",
                false => "",
            }
        ),
        cx,
    );
}

fn keycap(
    shortcut: &'static Shortcut,
    keys: Option<&str>,
    listening: bool,
    this: &Entity<Workbench>,
    t: &Tokens,
) -> AnyElement {
    let this = this.clone();
    let id = shortcut.id;
    let caret = div().child("_").with_animation(
        SharedString::from(format!("keycap-caret-{id}")),
        Animation::new(Duration::from_millis(800))
            .repeat()
            .with_max_fps(4.),
        |caret, delta| caret.opacity(if delta < 0.5 { 1. } else { 0. }),
    );
    div()
        .id(SharedString::from(format!("keycap-{id}")))
        .min_w(px(104.))
        .h(px(26.))
        .px(px(8.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .rounded(theme::RADIUS_2)
        .font_family(theme::MONO)
        .text_size(crate::theme::ui_text(11.))
        .cursor_pointer()
        .map(|cap| match (listening, keys.is_some()) {
            (true, _) => cap
                .border_color(t.border_selected)
                .bg(t.surface_selected)
                .text_color(t.accent)
                .child(div().flex().child("press keys").child(caret)),
            (false, filled) => cap
                .border_color(t.border)
                .bg(t.surface)
                .text_color(if filled {
                    t.text_primary
                } else {
                    t.text_tertiary
                })
                .hover(|style| style.border_color(t.border_strong))
                .child(keys_label(keys)),
        })
        .on_click(move |_, _, cx| this.update(cx, |bench, cx| record(bench, id, cx)))
        .into_any_element()
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.keybindings else {
        cx.defer_in(window, make);
        return Vec::new();
    };
    let t = Tokens::get(cx).clone();
    let overrides = bench.preferences().keybinding_overrides;
    let shortcuts = keymap::shortcuts();
    let query = fields.filter.read(cx).value().trim().to_lowercase();
    let recording = fields.recording.as_ref().map(|recording| recording.id);
    let matches = |shortcut: &Shortcut| {
        query.is_empty()
            || format!(
                "{} {} {}",
                shortcut.label,
                shortcut.detail,
                keys_label(shortcut.keys(&overrides).as_deref())
            )
            .to_lowercase()
            .contains(&query)
    };

    let groups = keymap::CATEGORIES.iter().filter_map(|category| {
        let listed: Vec<&'static Shortcut> = shortcuts
            .iter()
            .filter(|shortcut| shortcut.category == *category && matches(shortcut))
            .collect();
        if listed.is_empty() {
            return None;
        }
        let count = listed.len();
        let listed = listed.into_iter().map(|shortcut| {
            let keys = shortcut.keys(&overrides);
            let changed = overrides.contains_key(shortcut.id);
            let reset = changed.then(|| {
                let this = this.clone();
                ui::IconButton::new(
                    SharedString::from(format!("keycap-reset-{}", shortcut.id)),
                    Icon::Refresh,
                    format!("Reset {}", shortcut.label),
                )
                .small()
                .on_click(move |_, _, cx| {
                    this.update(cx, |bench, cx| {
                        change(bench, |overrides| {
                            overrides.remove(shortcut.id);
                        });
                        let restored = keys_label(shortcut.default_keys());
                        say(
                            bench,
                            format!("{} restored to {restored}.", shortcut.label),
                            cx,
                        );
                    })
                })
            });
            rows::row_with(
                None,
                shortcut.label,
                Some(shortcut.detail.into()),
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(reset)
                    .child(keycap(
                        shortcut,
                        keys.as_deref(),
                        recording == Some(shortcut.id),
                        this,
                        &t,
                    )),
                cx,
            )
        });
        Some(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.))
                        .px(px(14.))
                        .pt(px(12.))
                        .pb(px(4.))
                        .child(
                            div()
                                .text_size(crate::theme::ui_text(11.))
                                .text_color(t.text_secondary)
                                .child(*category),
                        )
                        .child(
                            div()
                                .text_size(crate::theme::ui_text(10.5))
                                .text_color(t.text_tertiary)
                                .child(count.to_string()),
                        ),
                )
                .children(listed),
        )
    });
    let groups: Vec<_> = groups.collect();
    let nothing = groups.is_empty() && !query.is_empty();

    let changed = overrides.len();
    let reset_all = {
        let this = this.clone();
        ui::Button::new("keycap-reset-all", "Reset all", ui::Variant::Outline)
            .icon(Icon::Refresh)
            .disabled(changed == 0)
            .on_click(move |_, _, cx| {
                this.update(cx, |bench, cx| {
                    change(bench, BTreeMap::clear);
                    stop(
                        bench,
                        "All shortcuts restored to their defaults.".into(),
                        cx,
                    );
                })
            })
    };
    let toolbar = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .p(px(14.))
        .border_b_1()
        .border_color(t.line_soft)
        .child(
            div()
                .flex_1()
                .child(ui::Field::new(&fields.filter).icon(Icon::Search)),
        )
        .child(reset_all);
    let message = fields
        .message
        .clone()
        .unwrap_or_else(|| "Select any keycap to record a replacement.".into());
    let status = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(14.))
        .py(px(8.))
        .border_b_1()
        .border_color(t.line_soft)
        .text_size(crate::theme::ui_text(12.))
        .text_color(t.text_secondary)
        .child(
            div()
                .size(px(6.))
                .rounded_full()
                .bg(if recording.is_some() {
                    t.accent
                } else {
                    t.text_disabled
                }),
        )
        .child(message);
    let system = match cfg!(target_os = "macos") {
        true => "macOS may keep system-reserved combinations before Sikemux can receive them.",
        false => "Windows may keep system-reserved combinations before Sikemux can receive them.",
    };
    let body = div()
        .child(toolbar)
        .child(status)
        .children(groups)
        .when(nothing, |body| {
            body.child(rows::note(format!("No commands match “{}”.", query), cx))
        })
        .child(rows::note(system, cx));
    vec![rows::section(
        "Shortcuts",
        Some(format!("{} commands · {changed} changed", shortcuts.len()).into()),
        Some("Select a keycap, then press a new combination. Conflicts are blocked.".into()),
        lit("Shortcuts"),
        body,
        cx,
    )]
}
