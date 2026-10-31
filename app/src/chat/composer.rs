use std::cell::Cell;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::base::input::InlineTokenContext;
use gpui_kit::base::Button as BaseButton;
use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, size, Anchor, AnyElement, AnyView, App, ClipboardItem, ElementId, Entity,
    Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Transformation, Window,
};
use sikemux_wire::chat::{Compactions, ContextUsage};
use sikemux_wire::protocol::ConfigPicker;

use crate::chat::model_picker::Picked;
use crate::icons::Icon;
use crate::motion::{Curve, Ease};
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4, TEXT_2XS, TEXT_2XS_H,
    TEXT_3XS_H, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, DockMenu, IconButton};
use crate::workbench::Workbench;

pub type Act = Rc<dyn Fn(&mut Window, &mut App)>;

/// The line under the composer: why the last message did not go, with what to do about it, or
/// what it waits for.
pub enum Notice {
    Refused(SharedString, Option<(SharedString, Act)>),
    Waiting(SharedString),
    /// Something just done to the draft, and how to take it back.
    Done(SharedString, (SharedString, Act)),
}
type Pick = Rc<dyn Fn(&str, &str, &mut Window, &mut App)>;
type Paste = Rc<dyn Fn(&ClipboardItem, &mut Window, &mut App) -> bool>;
type Chip = Rc<dyn Fn(&InlineTokenContext, &mut Window, &mut App) -> AnyElement>;

/// Where the chat runs, docked on its composer: the host, the project and its branch or
/// worktree on the left, the account on the right.
pub struct Strip {
    /// The account host the chat runs on, or none for this one.
    pub host: Option<SharedString>,
    pub project: SharedString,
    /// The project's folder, which its name's tooltip gives.
    pub path: SharedString,
    pub branch: Option<SharedString>,
    /// Until the first message the project's name opens a switcher that moves the chat: the
    /// chat, the folder it is in now, and the workbench that moves it.
    pub moves: Option<(String, String, Entity<Workbench>)>,
    pub worktree: Option<AnyElement>,
    /// A worktree agent's pull request and its Remove worktree, beside the worktree.
    pub pull: Option<AnyElement>,
    pub account: Option<AnyElement>,
}

/// A chat's composer: its draft, and the bar of what the chat lets the person change, as the
/// chat says. It keeps no state of its own.
#[derive(IntoElement)]
pub struct Composer {
    draft: Entity<TextareaState>,
    provider: String,
    agent_label: SharedString,
    strip: Option<Strip>,
    /// Something is docked on the strip, which then meets it square rather than rounding.
    covered: bool,
    ready: bool,
    running: bool,
    steers: bool,
    yolo: Option<(bool, bool, Act)>,
    pickers: Option<(Option<ConfigPicker>, Option<ConfigPicker>, Pick)>,
    /// The model menu, which before the chat starts also moves it to another agent or paying
    /// provider; the strip picks the account.
    model_menu: Option<(Picked, Entity<Workbench>)>,
    usage: Option<ContextUsage>,
    compactions: Option<Compactions>,
    /// The files going with the message, the button that picks more, what a paste of
    /// files or pictures does, and how the draft draws the tags that name them.
    attachments: Option<(Option<AnyElement>, Act, Paste, Chip)>,
    /// What the word at the caret is offered, docked over the top edge.
    menu: Option<DockMenu>,
    /// The Stash's button: how many items are new, whether its tray is open, and what opens it.
    stash: Option<(usize, bool, Act)>,
    /// The Stash's tray, docked over the top edge.
    tray: Option<AnyElement>,
    notice: Option<Notice>,
    /// The mic, and what it has heard so far.
    dictation: Option<AnyView>,
    heard: Option<SharedString>,
    /// The chat's text size, as a share of its designed size.
    text_scale: f32,
    on_send: Act,
    on_stop: Act,
}

impl Composer {
    pub fn new(
        draft: &Entity<TextareaState>,
        provider: &str,
        agent_label: impl Into<SharedString>,
        on_send: Act,
        on_stop: Act,
    ) -> Self {
        Self {
            draft: draft.clone(),
            provider: provider.to_owned(),
            agent_label: agent_label.into(),
            strip: None,
            covered: false,
            ready: false,
            running: false,
            steers: true,
            yolo: None,
            pickers: None,
            model_menu: None,
            usage: None,
            compactions: None,
            attachments: None,
            menu: None,
            stash: None,
            tray: None,
            notice: None,
            dictation: None,
            heard: None,
            text_scale: 1.,
            on_send,
            on_stop,
        }
    }

    pub fn strip(mut self, strip: Option<Strip>) -> Self {
        self.strip = strip;
        self
    }

    pub fn covered(mut self, covered: bool) -> Self {
        self.covered = covered;
        self
    }

    /// The session takes changes to its mode and settings.
    pub fn ready(mut self, ready: bool) -> Self {
        self.ready = ready;
        self
    }

    pub fn running(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    /// Sending while a turn runs steers into it, else it queues behind it.
    pub fn steers(mut self, steers: bool) -> Self {
        self.steers = steers;
        self
    }

    /// Whether the agent runs without asking, for one that can switch, and how to switch it.
    /// The switch between asking before acting and not; `held` while a change of it is
    /// still on its way or the agent waits on a permission.
    pub fn yolo(mut self, on: bool, held: bool, toggle: Act) -> Self {
        self.yolo = Some((on, held, toggle));
        self
    }

    pub fn pickers(
        mut self,
        model: Option<ConfigPicker>,
        effort: Option<ConfigPicker>,
        pick: Pick,
    ) -> Self {
        self.pickers = Some((model, effort, pick));
        self
    }

    pub fn model_menu(mut self, picked: Picked, bench: Entity<Workbench>) -> Self {
        self.model_menu = Some((picked, bench));
        self
    }

    pub fn attachments<E: IntoElement>(
        mut self,
        tiles: Option<AnyElement>,
        on_add: Act,
        on_paste: Paste,
        chip: impl Fn(&InlineTokenContext, &mut Window, &mut App) -> E + 'static,
    ) -> Self {
        let chip: Chip =
            Rc::new(move |token, window, cx| chip(token, window, cx).into_any_element());
        self.attachments = Some((tiles, on_add, on_paste, chip));
        self
    }

    pub fn dictation(mut self, mic: impl Into<AnyView>, heard: Option<SharedString>) -> Self {
        (self.dictation, self.heard) = (Some(mic.into()), heard);
        self
    }

    pub fn text_scale(mut self, scale: f32) -> Self {
        self.text_scale = scale;
        self
    }

    pub fn notice(mut self, notice: Option<Notice>) -> Self {
        self.notice = notice;
        self
    }

    pub fn stash(mut self, fresh: usize, open: bool, toggle: Act) -> Self {
        self.stash = Some((fresh, open, toggle));
        self
    }

    pub fn tray(mut self, tray: Option<AnyElement>) -> Self {
        self.tray = tray;
        self
    }

    pub fn menu(mut self, menu: Option<DockMenu>) -> Self {
        self.menu = menu;
        self
    }

    pub fn usage(mut self, usage: Option<ContextUsage>, compactions: Option<Compactions>) -> Self {
        self.usage = usage;
        self.compactions = compactions;
        self
    }
}

impl RenderOnce for Composer {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let fill = t.surface_composer;
        let (tiles, on_add, on_paste, chip) = match self.attachments {
            Some((tiles, on_add, on_paste, chip)) => {
                (tiles, Some(on_add), Some(on_paste), Some(chip))
            }
            None => (None, None, None, None),
        };
        let drafted = tiles.is_some() || !self.draft.read(cx).value().trim().is_empty();
        let stopping = self.running && !drafted;
        let job = crate::motion::follow(
            ElementId::NamedInteger("composer-send-job".into(), self.draft.entity_id().as_u64()),
            if stopping { 1. } else { 0. },
            SEND_TURN,
            window,
            cx,
        );
        let focused = self.draft.focus_handle(cx).contains_focused(window, cx);
        let edge = crate::motion::fade(
            &ElementId::NamedInteger("composer-edge".into(), self.draft.entity_id().as_u64()),
            "border",
            if focused { t.border_selected } else { t.border },
            FOCUS_EDGE,
            window,
            cx,
        );
        let settable = self.ready && !self.running;
        let width = window
            .use_keyed_state(
                ElementId::NamedInteger("composer-width".into(), self.draft.entity_id().as_u64()),
                cx,
                |_, _| Rc::new(Cell::new(0.)),
            )
            .read(cx)
            .clone();
        let room = Room::of(width.get());
        let attach = IconButton::new("composer-attach", Icon::Plus, "Add files")
            .glyph(17.)
            .width(31.)
            .when_some(on_add, |button, on_add| {
                button.on_click(move |_, window, cx| on_add(window, cx))
            });
        let mut bar = div()
            .min_h(px(34.))
            .pt(SPACE_2)
            .px(px(3.))
            .pb(px(1.))
            .flex()
            .items_center()
            .gap(px(5.))
            .child(attach)
            .children(
                self.stash
                    .map(|(fresh, open, toggle)| stash_button(fresh, open, toggle, room, cx)),
            );
        if let Some((on, held, toggle)) = self.yolo {
            bar = bar.child(yolo(
                on,
                settable && !held,
                &self.agent_label,
                toggle,
                room,
                cx,
            ));
        }
        let waiting = self
            .model_menu
            .as_ref()
            .is_some_and(|(picked, _)| !picked.locked);
        let model_menu = self.model_menu.map(|(picked, bench)| {
            let enabled = settable || !picked.locked;
            let label = picked.model.as_ref().map_or_else(
                || self.agent_label.to_string(),
                |model| chosen(model, "Model"),
            );
            let trigger = picker_trigger(
                "model",
                label,
                "Model",
                Some(&self.provider),
                enabled,
                room,
                cx,
            );
            crate::chat::model_picker::around(trigger, &self.provider, picked, &bench)
                .min_w_0()
                .flex()
        });
        let has_menu = model_menu.is_some();
        bar = bar.children(model_menu);
        if let Some((model, effort, pick)) = self.pickers {
            bar = bar.children(model.filter(|_| !has_menu).map(|model| {
                picker(
                    model,
                    "Model",
                    Some(&self.provider),
                    settable,
                    pick.clone(),
                    room,
                    cx,
                )
            }));
            bar = bar.children(effort.map(|effort| {
                picker(
                    effort,
                    "Reasoning effort",
                    None,
                    settable || waiting,
                    pick,
                    room,
                    cx,
                )
            }));
        }
        let bar = bar
            .children(
                self.usage
                    .map(|usage| context_meter(usage, self.compactions, &self.provider, cx)),
            )
            .child(div().flex_1())
            .children(self.dictation)
            .child(send(
                stopping,
                drafted,
                self.running.then_some(self.steers),
                self.on_send,
                self.on_stop,
                job,
                cx,
            ));
        let has_strip = self.strip.is_some();
        div()
            .w_full()
            .px(SPACE_4)
            .pb(SPACE_4)
            .flex()
            .flex_col()
            .items_center()
            .on_children_prepainted(move |bounds, window, cx| {
                let composer = bounds
                    .get(usize::from(has_strip))
                    .map_or(0., |box_| box_.size.width.into());
                if Room::of(composer) != Room::of(width.replace(composer)) {
                    window.refresh();
                }
                Drawn::keep(cx, |drawn| Drawn {
                    composer,
                    strip: if has_strip { drawn.strip } else { 0. },
                });
            })
            .children(
                self.strip
                    .map(|strip| project_strip(strip, self.covered, fill, cx)),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(px(820.))
                    .p(SPACE_2)
                    .flex()
                    .flex_col()
                    .border_1()
                    .border_color(edge)
                    .rounded(theme::RADIUS_3)
                    .bg(fill)
                    .children(tiles)
                    .child(
                        div()
                            .py(SPACE_0)
                            .min_h(px(14.5 * self.text_scale * 1.75 * 2.5))
                            .child(
                                Textarea::new(&self.draft)
                                    .appearance(false)
                                    .text_size(px(14.5 * self.text_scale))
                                    .line_height(gpui_kit::relative(1.75))
                                    .text_color(t.text_primary)
                                    .when_some(on_paste, |draft, on_paste| {
                                        draft.on_paste(move |item, window, cx| {
                                            on_paste(item, window, cx)
                                        })
                                    })
                                    .when_some(chip, |draft, chip| {
                                        draft
                                            .token(move |token, window, cx| chip(token, window, cx))
                                    }),
                            ),
                    )
                    .children(self.heard.map(|heard| {
                        div()
                            .px(SPACE_2H)
                            .pb(SPACE_1)
                            .text_size(px(14.5 * self.text_scale))
                            .text_color(t.text_disabled)
                            .child(heard)
                    }))
                    .child(bar)
                    .children(self.menu)
                    .children(self.tray.map(|tray| ui::dock_shell(cx).child(tray))),
            )
            .children(self.notice.map(|notice| {
                let line = div()
                    .w_full()
                    .max_w(px(820.))
                    .pt(SPACE_1H)
                    .px(SPACE_1)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
                    .text_size(crate::theme::ui_text(TEXT_XS));
                match notice {
                    Notice::Refused(words, action) => line
                        .text_color(t.danger)
                        .child(words)
                        .children(action.map(|(label, act)| {
                            ui::sign_in::link("composer-notice-action", label, cx)
                                .on_click(move |_, window, cx| act(window, cx))
                        })),
                    Notice::Waiting(words) => {
                        line.child(ui::shimmer::shimmer("composer-notice-waiting", words))
                    }
                    Notice::Done(words, (label, act)) => line
                        .text_color(t.text_secondary)
                        .child(ui::icon(Icon::Check, 12.).text_color(t.live))
                        .child(words)
                        .child(div().text_color(t.text_disabled).child("·"))
                        .child(
                            ui::sign_in::link("composer-notice-undo", label, cx)
                                .on_click(move |_, window, cx| act(window, cx)),
                        ),
                }
            }))
    }
}

fn project_strip(strip: Strip, covered: bool, fill: Hsla, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let name = |chevron: bool| {
        div()
            .h(px(26.))
            .px(SPACE_2)
            .max_w(px(240.))
            .flex()
            .items_center()
            .gap(px(7.))
            .rounded_full()
            .text_size(crate::theme::ui_text(TEXT_XS))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.text_primary)
            .child(ui::icon(Icon::Folder, 12.).text_color(t.live))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(strip.project.clone()),
            )
            .when(chevron, |name| {
                name.child(
                    ui::icon(Icon::Chevron, 10.)
                        .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                        .text_color(t.text_tertiary),
                )
            })
    };
    let project = match strip.moves {
        Some((chat, cwd, bench)) => crate::project_switcher::around(
            BaseButton::new("composer-project")
                .accessibility_label("Project")
                .min_w_0()
                .rounded_full()
                .hover(move |style| style.bg(t.surface_hover))
                .tooltip(ui::tooltip(strip.path.clone()))
                .child(name(true)),
            &chat,
            &cwd,
            &bench,
        )
        .into_any_element(),
        None => div()
            .id("composer-project")
            .min_w_0()
            .tooltip(ui::tooltip(strip.path.clone()))
            .child(name(false))
            .into_any_element(),
    };
    // As wide as the composer under it less a step either side, so it sits on it like a tab.
    let strip_row = div()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .gap(SPACE_0)
        .py(SPACE_1)
        .px(SPACE_1H)
        .border_t_1()
        .border_l_1()
        .border_r_1()
        .border_color(t.border)
        .when(!covered, |row| row.rounded_t(theme::RADIUS_3))
        .bg(fill)
        .child(
            div()
                .min_w_0()
                .max_w(px(180.))
                .px(SPACE_2)
                .flex()
                .items_center()
                .gap(SPACE_1H)
                .child(match strip.host {
                    Some(_) => ui::icon(Icon::Server, 12.).flex_none().text_color(t.host),
                    None => ui::icon(Icon::Laptop, 12.)
                        .flex_none()
                        .text_color(t.text_secondary),
                })
                .child(
                    ui::text(
                        strip.host.clone().unwrap_or_else(|| "This host".into()),
                        TEXT_XS,
                        FontWeight::NORMAL,
                    )
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_secondary),
                ),
        )
        .child(
            ui::icon(Icon::Chevron, 10.)
                .flex_none()
                .text_color(t.text_tertiary),
        )
        .child(project)
        .when(strip.branch.is_some() || strip.worktree.is_some(), |row| {
            row.child(
                div()
                    .flex_none()
                    .w(px(1.))
                    .h(px(14.))
                    .mx(SPACE_1H)
                    .bg(t.border),
            )
        })
        .children(strip.branch.map(|branch| {
            div()
                .min_w_0()
                .max_w(px(200.))
                .px(SPACE_2)
                .flex()
                .items_center()
                .gap(px(5.))
                .child(ui::icon(Icon::Git, 11.).text_color(t.text_tertiary))
                .child(
                    ui::mono(branch, TEXT_3XS_H, cx)
                        .text_color(t.text_tertiary)
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap(),
                )
        }))
        .children(strip.worktree)
        .children(strip.pull)
        .child(div().flex_1())
        .children(
            strip
                .account
                .map(|account| div().min_w_0().flex().child(account)),
        );
    div()
        .w_full()
        .max_w(px(820.))
        .px(px(STRIP_STEP))
        .child(strip_row)
        .on_children_prepainted(|bounds, _, cx| {
            let strip = bounds.first().map_or(0., |row| row.size.width.into());
            Drawn::keep(cx, |drawn| Drawn { strip, ..drawn });
        })
}

/// The widths the composer and the strip over it were last drawn at, which the shot example
/// reports so a check can hold the strip narrower than the composer.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Drawn {
    pub strip: f32,
    pub composer: f32,
}

impl gpui_kit::Global for Drawn {}

impl Drawn {
    /// Sets the widths only when they changed, as setting a global wakes whatever watches it.
    fn keep(cx: &mut App, next: impl FnOnce(Drawn) -> Drawn) {
        let drawn = cx.try_global::<Drawn>().copied().unwrap_or_default();
        let next = next(drawn);
        if next != drawn {
            cx.set_global(next);
        }
    }
}

/// The Stash's button beside +: its mark and name, and how many items are new in an accent pill.
fn stash_button(fresh: usize, open: bool, toggle: Act, room: Room, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    BaseButton::new("composer-stash")
        .accessibility_label("Stash")
        .h(px(28.))
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded(theme::RADIUS_2)
        .text_size(crate::theme::ui_text(TEXT_XS_H))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if open {
            t.text_primary
        } else {
            t.text_secondary
        })
        .when(open, |button| button.bg(t.surface_chosen))
        .hover(move |button| button.bg(t.surface_hover).text_color(t.text_primary))
        .tooltip(ui::tooltip("Stash · &"))
        .on_click(move |_, window, cx| toggle(window, cx))
        .child(ui::icon(Icon::Stack, 14.).text_color(if open {
            t.text_primary
        } else {
            t.text_secondary
        }))
        .when(room == Room::Full, |button| button.child("Stash"))
        .when(fresh > 0, |button| {
            button.child(
                div()
                    .h(px(16.))
                    .min_w(px(16.))
                    .px(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(t.accent)
                    .text_size(crate::theme::ui_text(TEXT_3XS_H))
                    .font_weight(FontWeight::BOLD)
                    .font_features(theme::tabular())
                    .text_color(t.on_accent)
                    .child(fresh.to_string()),
            )
        })
}

/// How much of the bar fits at the composer's width. The model's name shortens first; past
/// that the Stash and the effort lose their names, then YOLO, then the model's name goes.
#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Room {
    Bare,
    Tight,
    Compact,
    Full,
}

impl Room {
    /// The room at a composer drawn `width` wide, or all of it before it is first drawn.
    fn of(width: f32) -> Self {
        match width {
            w if w == 0. || w >= 500. => Room::Full,
            w if w >= 430. => Room::Compact,
            w if w >= 380. => Room::Tight,
            _ => Room::Bare,
        }
    }
}

/// How far the strip over the composer steps in from either side of it.
const STRIP_STEP: f32 = 20.;

/// YOLO or SAFE: whether the agent acts without asking. YOLO's letters run through the
/// rainbow the app paints it with.
fn yolo(
    on: bool,
    settable: bool,
    agent: &str,
    toggle: Act,
    room: Room,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let press = crate::keymap::hint("agent.permissions", cx).unwrap_or("Click".into());
    let about = if on {
        format!("YOLO mode on — {agent} runs without approvals. {press} turns it off.")
    } else {
        format!("Safe mode — {agent} asks before it acts. {press} goes YOLO.")
    };
    BaseButton::new("composer-yolo")
        .accessibility_label(if on { "YOLO" } else { "SAFE" })
        .selected(on)
        .disabled(!settable)
        .h(px(31.))
        .px(px(if room >= Room::Compact { 11. } else { 9.5 }))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded_full()
        .when(!settable, |toggle| toggle.opacity(0.42))
        .when(settable, |toggle| {
            toggle.hover(move |style| {
                style.bg(Hsla {
                    a: 0.07,
                    ..t.text_primary
                })
            })
        })
        .tooltip(ui::tooltip(about))
        .on_click(move |_, window, cx| toggle(window, cx))
        .child(
            ui::icon(if on { Icon::BoltFill } else { Icon::Lock }, 12.).text_color(if on {
                t.accent
            } else {
                t.text_tertiary
            }),
        )
        .when(room >= Room::Compact, |toggle| {
            toggle.child(match on {
                true => ui::rainbow("YOLO", 10.5, cx).into_any_element(),
                false => ui::text("SAFE", TEXT_3XS_H, FontWeight::SEMIBOLD)
                    .letter_spacing(0.08)
                    .text_color(t.text_tertiary)
                    .into_any_element(),
            })
        })
}

/// One of the session's settings, such as its model, with a menu of the values it takes.
/// What a choice is called, as its picker's trigger shows it.
fn chosen(picker: &ConfigPicker, name: &str) -> String {
    let label = picker
        .choices
        .iter()
        .find(|choice| choice.value == picker.current)
        .map_or_else(|| picker.current.clone(), |choice| choice.label.clone());
    if label.is_empty() {
        name.split(' ').next_back().unwrap_or(name).to_owned()
    } else {
        label
    }
}

/// The button a picker opens from: the agent's mark beside the first, the choice, a chevron.
fn picker_trigger(
    id: &str,
    label: String,
    name: &'static str,
    provider: Option<&str>,
    enabled: bool,
    room: Room,
    cx: &App,
) -> BaseButton {
    let t = Tokens::get(cx).clone();
    let trigger = BaseButton::new(SharedString::from(format!("picker-{id}")))
        .accessibility_label(name)
        .disabled(!enabled)
        .when(enabled, |trigger| {
            trigger.hover(move |style| {
                style.bg(Hsla {
                    a: 0.07,
                    ..t.text_primary
                })
            })
        });
    picker_face(trigger, label, provider, enabled, room, cx)
}

/// How a picker's trigger looks, on whichever button opens it: the agent's mark when it is
/// the first, what is chosen, and a chevron.
fn picker_face<E: Styled + ParentElement + IntoElement>(
    trigger: E,
    label: String,
    provider: Option<&str>,
    enabled: bool,
    room: Room,
    cx: &App,
) -> E {
    let t = Tokens::get(cx).clone();
    let first = provider.is_some();
    let named = match room {
        Room::Full => true,
        Room::Compact | Room::Tight => first,
        Room::Bare => false,
    };
    trigger
        .h(px(31.))
        .px(px(if named { 11. } else { 9.5 }))
        .max_w(px(220.))
        .when(first, |trigger| trigger.min_w_0())
        .when(!first, |trigger| trigger.flex_none())
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded_full()
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .text_color(if first {
            t.text_secondary
        } else {
            t.text_tertiary
        })
        .when(!enabled, |trigger| trigger.opacity(0.45))
        .child(match provider {
            Some(provider) => ui::mark(provider, 17.),
            None => ui::icon(Icon::Gauge, 14.)
                .text_color(t.text_tertiary)
                .into_any_element(),
        })
        .when(named, |trigger| {
            trigger.child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(label),
            )
        })
        .child(
            div().opacity(0.6).child(
                ui::icon(Icon::Chevron, 10.)
                    .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                    .text_color(t.text_tertiary),
            ),
        )
}

fn picker(
    picker: ConfigPicker,
    name: &'static str,
    provider: Option<&str>,
    settable: bool,
    pick: Pick,
    room: Room,
    cx: &App,
) -> impl IntoElement {
    let first = provider.is_some();
    let label = chosen(&picker, name);
    let enabled = settable && !picker.choices.is_empty();
    if !enabled {
        return picker_trigger(&picker.id, label, name, provider, enabled, room, cx)
            .into_any_element();
    }
    let ConfigPicker {
        id,
        current,
        choices,
    } = picker;
    let trigger = KitButton::new(SharedString::from(format!("picker-{id}")))
        .ghost()
        .compact()
        .accessibility_label(name);
    picker_face(trigger, label, provider, enabled, room, cx)
        .dropdown_menu_with_anchor(
            if first {
                Anchor::BottomLeft
            } else {
                Anchor::BottomRight
            },
            ui::menu(move |mut popup, _, cx| {
                for choice in &choices {
                    let (pick, id, value) = (pick.clone(), id.clone(), choice.value.clone());
                    popup = popup.item(
                        ui::MenuRow::new(choice.label.clone())
                            .when(choice.is_default, |row| row.hint("Default"))
                            .item(cx)
                            .checked(choice.value == current)
                            .on_click(move |_, window, cx| pick(&id, &value, window, cx)),
                    );
                }
                popup
            }),
        )
        .into_any_element()
}

/// How much of the context window the session has used, as a ring.
fn context_meter(
    usage: ContextUsage,
    compactions: Option<Compactions>,
    provider: &str,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let percent = if usage.size > 0. {
        (usage.used / usage.size * 100.).clamp(0., 100.)
    } else {
        0.
    };
    let signal = match percent {
        p if p >= 90. => t.danger,
        p if p >= 70. => t.warn,
        _ if matches!(provider, "claude" | "codex") => theme::brand(provider),
        _ => t.accent,
    };
    let percent_ink = if percent >= 70. {
        signal
    } else {
        t.text_primary
    };
    div()
        .id("context-meter")
        .size(px(31.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .hover(move |style| {
            style.bg(Hsla {
                a: 0.07,
                ..t.text_primary
            })
        })
        .tooltip(ui::card_tooltip(236., {
            let compactions = compactions.clone();
            move |cx| {
                context_card(
                    usage.clone(),
                    compactions.clone(),
                    percent,
                    signal,
                    percent_ink,
                    cx,
                )
            }
        }))
        .child(match compactions {
            None => ui::ring(Some((percent, signal)), 14., 1.9, t.line).into_any_element(),
            Some(compactions) => counted_ring(percent, signal, compactions.count, &t),
        })
}

/// The context ring grown to hold how often the conversation was compacted.
fn counted_ring(percent: f64, signal: Hsla, count: u32, t: &Tokens) -> AnyElement {
    let size = if count < 10 { 10. } else { 8.5 };
    div()
        .relative()
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .absolute()
                .inset_0()
                .child(ui::ring(Some((percent, signal)), 18., 2., t.line)),
        )
        .child(
            ui::text(count.to_string(), size, FontWeight::SEMIBOLD)
                .font_features(theme::tabular())
                .text_color(t.text_primary),
        )
        .into_any_element()
}

const CONTEXT_SEGMENTS: usize = 24;

/// The context meter's tooltip: the share used as a row of segments, then tokens, compactions
/// and cost.
fn context_card(
    usage: ContextUsage,
    compactions: Option<Compactions>,
    percent: f64,
    signal: Hsla,
    percent_ink: Hsla,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx);
    let lit = (percent / 100. * CONTEXT_SEGMENTS as f64).round() as usize;
    let unlit = Hsla {
        a: 0.07,
        ..t.text_primary
    };
    let row = |label: &str, value: AnyElement| {
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                ui::text(label.to_owned(), TEXT_2XS_H, FontWeight::NORMAL)
                    .text_color(t.text_tertiary),
            )
            .child(value)
    };
    let tokens = div()
        .flex()
        .gap(px(3.))
        .font_features(theme::tabular())
        .child(
            ui::text(
                crate::home::compact(usage.used),
                TEXT_2XS_H,
                FontWeight::NORMAL,
            )
            .text_color(t.text_primary),
        )
        .child(
            ui::text(
                format!("/ {}", crate::home::compact(usage.size)),
                TEXT_2XS_H,
                FontWeight::NORMAL,
            )
            .text_color(t.text_tertiary),
        );
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    ui::text("Context window", TEXT_XS, FontWeight::NORMAL)
                        .text_color(t.text_secondary),
                )
                .child(
                    ui::text(
                        format!("{}%", percent.round()),
                        TEXT_XS,
                        FontWeight::SEMIBOLD,
                    )
                    .font_features(theme::tabular())
                    .text_color(percent_ink),
                ),
        )
        .child(div().mt(px(9.)).mb(SPACE_2).flex().gap(SPACE_0).children(
            (0..CONTEXT_SEGMENTS).map(|segment| {
                div()
                    .flex_1()
                    .h(px(10.))
                    .rounded(px(1.5))
                    .bg(if segment < lit { signal } else { unlit })
            }),
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(row("Tokens", tokens.into_any_element()))
                .children(compactions.map(|compactions| {
                    let ink = match compactions.count {
                        10.. => t.danger,
                        5.. => t.warn,
                        _ => t.text_primary,
                    };
                    let times = match compactions.count {
                        1 => "Once".to_owned(),
                        count => format!("{count} times"),
                    };
                    row(
                        "Compacted",
                        ui::text(
                            format!("{times} · {}", ui::time::ago(compactions.last_at)),
                            TEXT_2XS_H,
                            FontWeight::NORMAL,
                        )
                        .font_features(theme::tabular())
                        .text_color(ink)
                        .into_any_element(),
                    )
                }))
                .children(usage.cost.map(|cost| {
                    row(
                        "Session cost",
                        ui::text(
                            crate::home::money(cost.amount),
                            TEXT_2XS_H,
                            FontWeight::NORMAL,
                        )
                        .font_features(theme::tabular())
                        .text_color(t.text_primary)
                        .into_any_element(),
                    )
                })),
        )
        .into_any_element()
}

/// How the composer's edge changes colour as it takes and loses the keys.
const FOCUS_EDGE: Ease = Ease::new(140, Curve::OUT);

/// How the send button's new mark turns in.
const SEND_TURN: Ease = Ease::new(150, Curve::OUT);

/// Send, or while a turn runs and nothing is written, Stop. While a turn runs, `steers` tells
/// whether sending steers into it. `job` runs from 0 (send) to 1 (stop): when the button
/// changes job, its new mark turns in.
fn send(
    stopping: bool,
    drafted: bool,
    steers: Option<bool>,
    on_send: Act,
    on_stop: Act,
    job: f32,
    cx: &App,
) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let (mark, turned, from) = match stopping {
        true => (ui::icon(Icon::Stop, 18.), job, -FRAC_PI_2),
        false => (ui::icon(Icon::ArrowUp, 15.), 1. - job, FRAC_PI_2),
    };
    let mark = mark
        .text_color(t.surface_sunken)
        .opacity(turned)
        .with_transformation(
            Transformation::rotate(radians(from * (1. - turned)))
                .with_scaling(size(0.5 + 0.5 * turned, 0.5 + 0.5 * turned)),
        );
    let about = steers.filter(|_| !stopping).map(|steers| {
        if steers {
            "Steers into this turn — ⌘↵ queues behind it"
        } else {
            "Queues behind this turn — ⌘↵ steers into it"
        }
    });
    BaseButton::new("composer-send")
        .accessibility_label(if stopping {
            "Stop agent"
        } else {
            "Send message"
        })
        .disabled(!stopping && !drafted)
        .ml(px(3.))
        .size(px(30.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(t.text_primary)
        .when(!stopping && !drafted, |button| button.opacity(0.28))
        .when_some(about, |button, about| button.tooltip(ui::tooltip(about)))
        .on_click(move |_, window, cx| {
            if stopping {
                on_stop(window, cx)
            } else {
                on_send(window, cx)
            }
        })
        .child(mark)
}
