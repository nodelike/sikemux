//! The Mac's Edit Spaces…: each space as a row to drag into place, mark with an icon, rename
//! or delete, with New and Done at its foot.

use std::collections::HashMap;

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::Sizable;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, App, AppContext, Context, Entity, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription,
    WeakEntity, Window,
};
use sikemux_wire::protocol::{ProjectInfo, SpaceInfo};

use crate::icons::Icon;
use crate::theme::{Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_2, TEXT_3XS_H, TEXT_XS};
use crate::ui;
use crate::ui::IconButton;
use crate::workbench::{Workbench, MAX_SPACE_NAME};
use sikemux_core::server::call;
use sikemux_core::server::workspace::{SpacesParams, UpdateProjectParams};

/// The icons a space can wear, as the Mac offers them.
pub const SPACE_ICONS: [&str; 18] = [
    "folder",
    "user",
    "globe",
    "laptop",
    "building",
    "home",
    "briefcase",
    "code",
    "flask",
    "rocket",
    "star",
    "heart",
    "book",
    "bolt",
    "leaf",
    "cube",
    "terminal",
    "music",
];

/// The icons `space` can wear, the one it wears ticked, then No icon.
pub fn icon_choices(
    mut menu: PopupMenu,
    bench: WeakEntity<Workbench>,
    space: &str,
    worn: &str,
) -> PopupMenu {
    let choice = |label: &'static str, icon: &'static str| {
        let (bench, space) = (bench.clone(), space.to_owned());
        let item = PopupMenuItem::new(label)
            .checked(worn == icon)
            .on_click(move |_, _, cx| {
                let _ = bench.update(cx, |bench, _| bench.set_space_icon(&space, icon));
            });
        match Icon::named(icon) {
            Some(glyph) => item.icon(glyph),
            None => item,
        }
    };
    for icon in SPACE_ICONS {
        menu = menu.item(choice(icon, icon));
    }
    menu.separator().item(choice("No icon", ""))
}

pub struct SpaceEditor {
    bench: WeakEntity<Workbench>,
    /// Each space's name field, by space, and what saves it.
    names: HashMap<String, (Entity<InputState>, Subscription)>,
}

impl SpaceEditor {
    pub fn new(bench: WeakEntity<Workbench>) -> Self {
        Self {
            bench,
            names: HashMap::new(),
        }
    }

    fn name_field(
        &mut self,
        id: &str,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some((field, _)) = self.names.get(id) {
            return field.clone();
        }
        let field = cx.new(|cx| InputState::new(window, cx).default_value(name.to_owned()));
        let (bench, space) = (self.bench.clone(), id.to_owned());
        let saving = cx.subscribe_in(&field, window, move |_, field, event, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                let typed = field.read(cx).value().to_string();
                let _ = bench.update(cx, |bench, _| bench.name_space(&space, &typed));
            }
        });
        self.names.insert(id.to_owned(), (field.clone(), saving));
        field
    }
}

/// A space being dragged to another place in the list.
#[derive(Clone)]
struct DraggedSpace {
    id: String,
    name: SharedString,
}

impl Render for DraggedSpace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx);
        div()
            .px(SPACE_2)
            .py(SPACE_1)
            .rounded(RADIUS_2)
            .bg(t.surface_raised)
            .border_1()
            .border_color(t.border)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_primary)
            .child(self.name.clone())
    }
}

impl Render for SpaceEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let spaces = self
            .bench
            .upgrade()
            .and_then(|bench| {
                let bench = bench.read(cx);
                Some(bench.this_view()?.workspace.spaces.clone())
            })
            .unwrap_or_default();
        self.names
            .retain(|id, _| spaces.iter().any(|space| &space.id == id));
        let rows: Vec<_> = spaces
            .iter()
            .enumerate()
            .map(|(at, space)| {
                let field = self.name_field(&space.id, &space.name, window, cx);
                let dragged = DraggedSpace {
                    id: space.id.clone(),
                    name: space.name.clone().into(),
                };
                let bench = self.bench.clone();
                let (worn, picked) = (space.icon.clone(), space.id.clone());
                let (deleted, removed) = (self.bench.clone(), space.id.clone());
                let row = SharedString::from(format!("space-edit-{}", space.id));
                let worn_glyph = Icon::named(&space.icon);
                div()
                    .id(row.clone())
                    .group(row.clone())
                    .h(px(30.))
                    .flex()
                    .items_center()
                    .gap(SPACE_1)
                    .px(SPACE_0)
                    .rounded(RADIUS_2)
                    .hover(move |row| row.bg(t.surface_hover))
                    .drag_over::<DraggedSpace>(move |row, _, _, _| row.bg(t.surface_hover))
                    .on_drop({
                        let bench = self.bench.clone();
                        move |dragged: &DraggedSpace, _, cx| {
                            let _ =
                                bench.update(cx, |bench, _| bench.move_space_to(&dragged.id, at));
                        }
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("space-grip-{}", space.id)))
                            .w(px(14.))
                            .h(px(24.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_grab()
                            .child(ui::icon(Icon::Grip, 12.).text_color(t.text_tertiary))
                            .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone())),
                    )
                    .child(
                        KitButton::new(SharedString::from(format!("space-icon-{}", space.id)))
                            .ghost()
                            .compact()
                            .w(px(26.))
                            .h(px(24.))
                            .rounded(RADIUS_2)
                            .border_1()
                            .border_color(t.border)
                            .when(worn_glyph.is_none(), |button| button.border_dashed())
                            .child(match worn_glyph {
                                Some(glyph) => ui::icon(glyph, 13.).text_color(t.text_primary),
                                None => ui::icon(Icon::Plus, 11.).text_color(t.text_tertiary),
                            })
                            .dropdown_menu_with_anchor(
                                Anchor::TopLeft,
                                ui::menu(move |menu, _, _| {
                                    icon_choices(menu, bench.clone(), &picked, &worn)
                                }),
                            ),
                    )
                    .child(div().flex_1().min_w_0().child(Input::new(&field).small()))
                    .child(
                        div()
                            .opacity(0.)
                            .group_hover(row, |delete| delete.opacity(1.))
                            .child(
                                IconButton::new(
                                    SharedString::from(format!("space-delete-{}", space.id)),
                                    Icon::Trash,
                                    format!("Delete {}", space.name),
                                )
                                .on_click(move |_, _, cx| {
                                    let _ = deleted.update(cx, |bench, cx| {
                                        bench.delete_space(removed.clone(), cx)
                                    });
                                }),
                            ),
                    )
            })
            .collect();
        let (made, done) = (self.bench.clone(), self.bench.clone());
        let foot = |id: &'static str, glyph: Icon, label: &'static str, done: bool| {
            div()
                .id(id)
                .h(px(24.))
                .px(SPACE_2)
                .flex()
                .items_center()
                .gap(px(5.))
                .rounded(RADIUS_2)
                .border_1()
                .text_size(crate::theme::ui_text(TEXT_3XS_H))
                .map(|button| match done {
                    true => button
                        .border_color(t.accent)
                        .bg(t.accent)
                        .text_color(t.on_accent)
                        .child(ui::icon(glyph, 11.).text_color(t.on_accent)),
                    false => button
                        .border_color(t.border)
                        .text_color(t.text_secondary)
                        .hover(move |button| button.bg(t.surface_hover).text_color(t.text_primary))
                        .child(ui::icon(glyph, 11.).text_color(t.text_secondary)),
                })
                .child(label)
        };
        div()
            .p(SPACE_1)
            .flex()
            .flex_col()
            .child(div().flex().flex_col().children(rows))
            .child(
                div()
                    .mt(SPACE_1)
                    .pt(SPACE_1)
                    .px(SPACE_0)
                    .border_t_1()
                    .border_color(t.border)
                    .flex()
                    .justify_between()
                    .child(foot("space-edit-new", Icon::Plus, "New", false).on_click(
                        move |_, _, cx| {
                            let _ = made.update(cx, |bench, cx| bench.new_space(None, cx));
                        },
                    ))
                    .child(foot("space-edit-done", Icon::Check, "Done", true).on_click(
                        move |_, _, cx| {
                            let _ = done.update(cx, |bench, cx| bench.close_space_editor(cx));
                        },
                    )),
            )
    }
}

impl Workbench {
    /// Opens Edit Spaces under the space switch.
    pub fn open_space_editor(&mut self, cx: &mut Context<Self>) {
        let bench = cx.weak_entity();
        self.overlays.space_editor = Some(cx.new(|_| SpaceEditor::new(bench)));
        cx.notify();
    }

    pub fn close_space_editor(&mut self, cx: &mut Context<Self>) {
        self.overlays.space_editor = None;
        cx.notify();
    }

    /// Names the space, as its field in Edit Spaces does; an empty name keeps the old one.
    pub fn name_space(&self, id: &str, name: &str) {
        let name: String = name
            .trim()
            .chars()
            .take(crate::workbench::MAX_SPACE_NAME)
            .collect();
        if name.is_empty() {
            return;
        }
        self.edit_spaces(|spaces| {
            if let Some(space) = spaces.iter_mut().find(|space| space.id == id) {
                if space.name != name {
                    space.name = name;
                }
            }
        });
    }

    /// Moves the space to `to` along the switch.
    pub fn move_space_to(&self, id: &str, to: usize) {
        self.edit_spaces(|spaces| {
            let Some(at) = spaces.iter().position(|space| space.id == id) else {
                return;
            };
            let space = spaces.remove(at);
            spaces.insert(to.min(spaces.len()), space);
        });
    }
}

/// Where Edit Spaces opens: under the space switch, as wide as it.
pub fn anchor(bench: &Workbench, width: f32, cx: &mut Context<Workbench>) -> impl IntoElement {
    let editor = bench.overlays.space_editor.clone();
    let this = cx.weak_entity();
    gpui_kit::base::Popover::new("space-editor")
        .open(editor.is_some())
        .anchor(Anchor::TopLeft)
        .w_full()
        .trigger_with(|_, _, _| div().w_full().h(px(0.)).into_any_element())
        .on_open_change(move |open: &bool, _: &mut Window, cx: &mut App| {
            if !*open {
                let _ = this.update(cx, |bench, cx| bench.close_space_editor(cx));
            }
        })
        .content(move |_, _, cx| {
            let t = Tokens::get(cx);
            div()
                .w(px(width))
                .mt(SPACE_1)
                .rounded(RADIUS_2 + px(3.))
                .bg(t.surface_overlay)
                .border_1()
                .border_color(t.border)
                .when_some(editor.clone(), |pop, editor| pop.child(editor))
        })
}

impl Workbench {
    /// The space the rail shows, while it still exists.
    pub fn space(&self) -> Option<&SpaceInfo> {
        let spaces = &self.shown()?.workspace.spaces;
        spaces
            .iter()
            .find(|space| Some(&space.id) == self.space.as_ref())
    }

    /// Asks for a name and makes a space with it, as the Mac does, then shows it, or moves the
    /// project `into` it when one asked.
    pub fn new_space(&mut self, into: Option<String>, cx: &mut Context<Self>) {
        self.prompt(
            "Create space",
            "",
            move |bench, name, _, cx| {
                let name: String = name.trim().chars().take(MAX_SPACE_NAME).collect();
                if name.is_empty() {
                    return;
                }
                let made = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| since.as_nanos());
                let id = format!("space-{made:x}");
                let mut spaces = bench
                    .this_view()
                    .map_or_else(Vec::new, |view| view.workspace.spaces.clone());
                spaces.push(SpaceInfo {
                    id: id.clone(),
                    name,
                    icon: "folder".into(),
                });
                bench.call_typed::<call::SetSpaces>(SpacesParams { spaces });
                match &into {
                    Some(project) => bench.call_typed::<call::UpdateProject>(
                        UpdateProjectParams::space(project.clone(), Some(id)),
                    ),
                    None => bench.show_space(Some(id), cx),
                }
            },
            cx,
        );
        cx.notify();
    }

    /// Sets this host's spaces to what `edit` makes of them.
    pub(crate) fn edit_spaces(&self, edit: impl FnOnce(&mut Vec<SpaceInfo>)) {
        let mut spaces = self
            .this_view()
            .map_or_else(Vec::new, |view| view.workspace.spaces.clone());
        edit(&mut spaces);
        self.call_typed::<call::SetSpaces>(SpacesParams { spaces });
    }

    /// Asks for the space's new name, as the Mac does.
    pub fn rename_space(&mut self, id: String, cx: &mut Context<Self>) {
        let current = self
            .this_view()
            .and_then(|view| view.workspace.spaces.iter().find(|space| space.id == id))
            .map(|space| space.name.clone())
            .unwrap_or_default();
        self.prompt(
            "Rename space",
            current,
            move |bench, name, _, _| bench.name_space(&id, &name),
            cx,
        );
        cx.notify();
    }

    /// Marks the space with `icon`, or with none when it is empty.
    pub fn set_space_icon(&self, id: &str, icon: &str) {
        self.edit_spaces(|spaces| {
            if let Some(space) = spaces.iter_mut().find(|space| space.id == id) {
                space.icon = icon.to_owned();
            }
        });
    }

    /// Deletes the space once the person agrees; its projects stay open and show under All.
    pub fn delete_space(&mut self, id: String, cx: &mut Context<Self>) {
        let name = self
            .this_view()
            .and_then(|view| view.workspace.spaces.iter().find(|space| space.id == id))
            .map(|space| space.name.clone())
            .unwrap_or_default();
        self.confirm(
            format!("Delete “{name}”?"),
            "Its projects stay open and show under All.",
            "Delete",
            true,
            move |bench, yes, _, cx| {
                if !yes {
                    return;
                }
                bench.edit_spaces(|spaces| spaces.retain(|space| space.id != id));
                let held: Vec<String> = bench
                    .this_view()
                    .map(|view| {
                        view.workspace
                            .projects
                            .iter()
                            .filter(|project| project.space.as_ref() == Some(&id))
                            .map(|project| project.id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                for project in held {
                    bench.call_typed::<call::UpdateProject>(UpdateProjectParams::space(
                        project, None,
                    ));
                }
                if bench.space.as_ref() == Some(&id) {
                    bench.show_space(None, cx);
                }
            },
            cx,
        );
    }

    /// Shows one space's projects, or every project, and leaves a project the space hides
    /// for the first one it shows.
    pub fn show_space(&mut self, space: Option<String>, cx: &mut Context<Self>) {
        self.space = space;
        let shown = |project: &ProjectInfo| {
            self.space.is_none() || project.space.as_ref() == self.space.as_ref()
        };
        if !self.project().is_some_and(shown) {
            if let Some(first) = self.projects().iter().find(|project| shown(project)) {
                self.activate_project(&first.id);
            }
        }
        cx.notify();
    }
}
