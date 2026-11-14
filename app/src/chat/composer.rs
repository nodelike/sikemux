use crate::ui::Clicks;
use crate::workbench::actions;
use std::cell::Cell;
use std::f32::consts::FRAC_PI_2;
use std::rc::Rc;

use gpui_kit::base::input::InlineTokenContext;
use gpui_kit::base::{Button as BaseButton, Popover};
use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, radians, size, Anchor, AnyElement, AnyView, App, ClipboardItem, ElementId, Entity,
    Focusable, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce,
    SharedString, StatefulInteractiveElement, Styled, Transformation, Window,
};
use sikemux_view::chat::{Compactions, ContextUsage, ExpiredCache};
use sikemux_view::workspace::ConfigPicker;
use sikemux_wire::protocol::PermissionMode;

use crate::chat::model_picker::Picked;
use crate::icons::Icon;
use crate::motion::{Curve, Ease};
use crate::theme::{
    self, Tokens, RADIUS_3, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_3, SPACE_4,
    TEXT_2XS, TEXT_2XS_H, TEXT_3XS_H, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, DockMenu, IconButton};
use crate::workbench::Workbench;

pub type Act = Rc<dyn Fn(&mut Window, &mut App)>;
/// Turns summarising before the next message on or off.
pub type Summarise = Rc<dyn Fn(bool, &mut Window, &mut App)>;
pub type PickMode = Rc<dyn Fn(PermissionMode, &mut Window, &mut App)>;

/// The permission modes an agent can switch between, the one in force, and what picking one does;
/// `held` while a change is still on its way or the agent waits on a permission.
pub struct Modes {
    pub current: PermissionMode,
    pub offered: Vec<PermissionMode>,
    pub held: bool,
    pub pick: PickMode,
}

/// The line under the composer: why the last message did not go, with what to do about it, or
/// what it waits for.
pub enum Notice {
    Refused(SharedString, Option<(SharedString, Act)>),
    Waiting(SharedString),
    /// Something just done, and how to take it back.
    Done(SharedString, (SharedString, Act)),
}
type Pick = Rc<dyn Fn(&str, &str, &mut Window, &mut App)>;
type Paste = Rc<dyn Fn(&ClipboardItem, &mut Window, &mut App) -> bool>;
type Chip = Rc<dyn Fn(&InlineTokenContext, &mut Window, &mut App) -> AnyElement>;
type DrawAside = Box<dyn FnOnce(bool) -> AnyElement>;

/// Wraps a chip of the strip, such as the project's name, in the picker it opens.
pub type ChipMenu = Box<dyn FnOnce(BaseButton) -> Popover>;

/// How much room a control at the end of the bar needs.
#[derive(Clone, Copy)]
pub struct AsideRoom {
    /// The composer's width from which it keeps its words.
    pub labelled_from: f32,
    /// What it takes from the bar once it has dropped them.
    pub bare: f32,
}

/// Where the chat runs, docked on its composer: the host, the project and its branch or
/// worktree on the left, the account on the right.
pub struct Strip {
    /// The account host the chat runs on, or none for this one.
    pub host: Option<SharedString>,
    /// The picker the host opens, until the first message says where the work is.
    pub hosts: Option<ChipMenu>,
    pub project: SharedString,
    /// The project's folder, which its name's tooltip gives.
    pub path: SharedString,
    pub branch: Option<SharedString>,
    /// The chip of an own checkout's ports, beside its branch.
    pub ports: Option<AnyElement>,
    /// The switcher the project's name opens, until the first message says where the work is.
    pub moves: Option<ChipMenu>,
    pub worktree: Option<AnyElement>,
    /// A worktree agent's pull request and its Remove worktree, beside the worktree.
    pub pull: Option<AnyElement>,
    pub account: Option<AnyElement>,
    /// Why smart accounts started this new chat on its account, said beside the account until
    /// the first message.
    pub picked: Option<SharedString>,
    /// Nothing is sent yet, so the line sits on a new chat's glow and is drawn in full ink.
    pub fresh: bool,
}

/// A chat's composer: its draft, and the bar of what the chat lets the person change, as the
/// chat says. It keeps no state of its own.
/// Says the model menu opened or shut.
pub type Opened = Rc<dyn Fn(bool, &mut App)>;

#[derive(IntoElement)]
pub struct Composer {
    draft: Entity<TextareaState>,
    provider: String,
    agent_label: SharedString,
    strip: Option<Strip>,
    ready: bool,
    running: bool,
    steers: bool,
    modes: Option<Modes>,
    pickers: Option<(Option<ConfigPicker>, Option<ConfigPicker>, Pick)>,
    /// The model menu, which before the chat starts also moves it to another agent or paying
    /// provider; the strip picks the account.
    model_menu: Option<(Picked, Entity<Workbench>)>,
    model_open: Option<(bool, Opened)>,
    usage: Option<ContextUsage>,
    compactions: Option<Compactions>,
    /// The chat's prompt cache expired, and what flips summarising before the next message.
    expired: Option<(ExpiredCache, Summarise)>,
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
    /// What takes a SnapShot of another app's window into the draft.
    snapshot: Option<Act>,
    /// The mic, and what it has heard so far.
    dictation: Option<AnyView>,
    heard: Option<SharedString>,
    /// A control of the composer's owner at the end of the bar, before the mic and send, told
    /// whether it keeps its words.
    aside: Option<(AsideRoom, DrawAside)>,
    /// Controls of the composer's owner at the start of the bar, after the attach button.
    leading: Option<AnyElement>,
    /// Words on the send button in place of its arrow.
    send_label: Option<SharedString>,
    /// The chat's text size, as a share of its designed size.
    text_scale: f32,
    /// What the agent predicts the person sends next, which the empty draft says with Tab
    /// beside it.
    suggestion: Option<SharedString>,
    /// Stands one line tall until more is written, where it sits under a long list.
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
            ready: false,
            running: false,
            steers: true,
            modes: None,
            pickers: None,
            model_menu: None,
            model_open: None,
            usage: None,
            compactions: None,
            expired: None,
            attachments: None,
            menu: None,
            stash: None,
            tray: None,
            notice: None,
            snapshot: None,
            dictation: None,
            heard: None,
            aside: None,
            leading: None,
            send_label: None,
            text_scale: 1.,
            suggestion: None,
            on_send,
            on_stop,
        }
    }

    pub fn suggestion(mut self, suggestion: Option<SharedString>) -> Self {
        self.suggestion = suggestion;
        self
    }

    pub fn leading(mut self, leading: AnyElement) -> Self {
        self.leading = Some(leading);
        self
    }

    pub fn send_label(mut self, label: impl Into<SharedString>) -> Self {
        self.send_label = Some(label.into());
        self
    }

    pub fn strip(mut self, strip: Option<Strip>) -> Self {
        self.strip = strip;
        self
    }

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

    /// The switch between the agent's permission modes, for an agent that can switch;
    /// `held` while a change of it is still on its way or the agent waits on a permission.
    pub fn modes(mut self, modes: Modes) -> Self {
        self.modes = Some(modes);
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

    /// Holds the model menu open or shut from outside, as `/model` opens it.
    pub fn model_open(mut self, open: bool, opened: Opened) -> Self {
        self.model_open = Some((open, opened));
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

    pub fn snapshot(mut self, take: Act) -> Self {
        self.snapshot = Some(take);
        self
    }

    pub fn dictation(mut self, mic: impl Into<AnyView>, heard: Option<SharedString>) -> Self {
        (self.dictation, self.heard) = (Some(mic.into()), heard);
        self
    }

    pub fn aside<E: IntoElement>(
        mut self,
        room: AsideRoom,
        aside: impl FnOnce(bool) -> E + 'static,
    ) -> Self {
        self.aside = Some((room, Box::new(move |roomy| aside(roomy).into_any_element())));
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

    /// Says `notice` when the composer has nothing more pressing to say.
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

    pub fn expired(mut self, expired: Option<(ExpiredCache, Summarise)>) -> Self {
        self.expired = expired;
        self
    }
}

impl RenderOnce for Composer {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = Tokens::get(cx).clone();
        let fill = Hsla {
            a: t.surface_composer.a * COMPOSER_SOLID,
            ..t.surface_composer
        };
        let (tiles, on_add, on_paste, chip) = match self.attachments {
            Some((tiles, on_add, on_paste, chip)) => {
                (tiles, Some(on_add), Some(on_paste), Some(chip))
            }
            None => (None, None, None, None),
        };
        let drafted = tiles.is_some() || !self.draft.read(cx).value().trim().is_empty();
        let text_size = px(14.5 * self.text_scale);
        let suggestion = (self.suggestion)
            .filter(|_| self.draft.read(cx).value().is_empty())
            .map(|suggestion| tab_hint(suggestion, text_size, cx));
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
        let yolo_on =
            (self.modes.as_ref()).is_some_and(|modes| modes.current == PermissionMode::Bypass);
        let width = window
            .use_keyed_state(
                ElementId::NamedInteger("composer-width".into(), self.draft.entity_id().as_u64()),
                cx,
                |_, _| Rc::new(Cell::new(0.)),
            )
            .read(cx)
            .clone();
        let aside_room = self.aside.as_ref().map(|(room, _)| *room);
        let missing = [
            self.stash.is_some(),
            self.modes.is_some(),
            self.usage.is_some(),
            self.dictation.is_some(),
        ]
        .into_iter()
        .filter(|present| !present)
        .count() as f32;
        let crowd = (f32::from(u8::from(self.snapshot.is_some())) - missing) * CONTROL;
        let (room, labelled) = shape(width.get(), aside_room, crowd);
        let attach = IconButton::new("composer-attach", Icon::Plus, t!("Add files"))
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
        if let Some(modes) = self.modes {
            let settable = settable && !modes.held;
            bar = bar.child(mode_chip(modes, settable, &self.agent_label, room, cx));
        }
        let waiting = self
            .model_menu
            .as_ref()
            .is_some_and(|(picked, _)| !picked.locked);
        let model_menu = self.model_menu.map(|(picked, bench)| {
            let enabled = settable || !picked.locked;
            let label = (picked.model.as_ref())
                .filter(|model| !model.current.is_empty())
                .map_or_else(
                    || self.agent_label.to_string(),
                    |model| chosen(model, t!("Model")),
                );
            let effort = picked
                .effort
                .as_ref()
                .map(|effort| chosen(effort, t!("Effort")));
            let face = Face {
                label,
                provider: Some(&self.provider),
                effort,
            };
            let trigger = picker_trigger("model", face, t!("Model"), enabled, room, cx);
            let chat = self.draft.entity_id().to_string();
            let menu = crate::chat::model_picker::around(trigger, &chat, picked, &bench);
            match self.model_open.clone() {
                Some((open, opened)) => menu.open(open).on_open_change(
                    move |open: &bool, _: &mut Window, cx: &mut App| opened(*open, cx),
                ),
                None => menu,
            }
            .min_w_0()
            .flex()
        });
        let has_menu = model_menu.is_some();
        bar = bar.children(model_menu);
        if let Some((model, effort, pick)) = self.pickers {
            bar = bar.children(model.filter(|_| !has_menu).map(|model| {
                picker(
                    model,
                    t!("Model"),
                    Some(&self.provider),
                    settable,
                    pick.clone(),
                    room,
                    cx,
                )
            }));
            bar = bar.children(effort.filter(|_| !has_menu).map(|effort| {
                picker(
                    effort,
                    t!("Reasoning effort"),
                    None,
                    settable || waiting,
                    pick,
                    room,
                    cx,
                )
            }));
        }
        let bar = bar
            .children(self.usage.map(|usage| {
                context_meter(usage, self.compactions, self.expired, &self.provider, cx)
            }))
            .children(self.leading)
            .child(div().flex_1())
            .children(self.aside.map(|(_, aside)| aside(labelled)))
            .children(self.snapshot.map(|take| snapshot_button(take, cx)))
            .children(self.dictation)
            .child(match self.send_label {
                Some(label) => {
                    let on_send = self.on_send;
                    ui::Button::new(
                        "composer-labelled-send",
                        label,
                        if drafted {
                            ui::Variant::Primary
                        } else {
                            ui::Variant::Outline
                        },
                    )
                    .disabled(!drafted)
                    .on_click(move |_, window, cx| on_send(window, cx))
                    .into_any_element()
                }
                None => send(
                    stopping,
                    drafted,
                    self.running.then_some(self.steers),
                    self.on_send,
                    self.on_stop,
                    job,
                    cx,
                )
                .into_any_element(),
            });
        let has_strip = self.strip.is_some();
        let context = self.strip.map(|strip| context_line(strip, cx));
        div()
            .w_full()
            .px(SPACE_4)
            .pb(SPACE_4)
            .flex()
            .flex_col()
            .items_center()
            .on_children_prepainted(move |bounds, window, cx| {
                let composer = bounds.first().map_or(0., |box_| box_.size.width.into());
                if shape(composer, aside_room, crowd)
                    != shape(width.replace(composer), aside_room, crowd)
                {
                    crate::ui::redraw_after_paint(window, cx);
                }
                Drawn::keep(cx, |drawn| Drawn {
                    composer,
                    strip: if has_strip { drawn.strip } else { 0. },
                });
            })
            .children(self.notice.map(|notice| notice_line(notice, cx)))
            .child(
                div()
                    .relative()
                    .w_full()
                    .max_w(super::column::composer(cx))
                    .rounded(theme::RADIUS_3)
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .p(SPACE_2)
                            .flex()
                            .flex_col()
                            .border_1()
                            .border_color(if yolo_on {
                                Hsla {
                                    a: YOLO_EDGE,
                                    ..t.accent
                                }
                            } else {
                                edge
                            })
                            .rounded(theme::RADIUS_3)
                            .bg(fill)
                            .backdrop_blur(px(COMPOSER_BLUR))
                            .children(tiles)
                            .child(
                                div()
                                    .relative()
                                    .py(SPACE_0)
                                    .min_h(px(14.5 * self.text_scale * 1.75 * 2.5))
                                    .children(suggestion)
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
                                                draft.token(move |token, window, cx| {
                                                    chip(token, window, cx)
                                                })
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
                    ),
            )
            .children(context)
    }
}

/// How far, as a share of their size, a line's letters sit below the middle of its line box,
/// so a key centred on the line is moved down onto them.
const GLYPHS_BELOW_MIDDLE: f32 = 0.17;

/// Tab's key after the suggestion the empty draft shows as its placeholder. The words are drawn
/// again unseen where the draft draws them, inside its padding, so the key follows them.
fn tab_hint(suggestion: SharedString, text_size: gpui_kit::Pixels, cx: &App) -> gpui_kit::Div {
    div()
        .absolute()
        .top(px(8.))
        .left(px(10.))
        .right(px(10.))
        .flex()
        .items_center()
        .gap_x(SPACE_1H)
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(text_size)
                .line_height(gpui_kit::relative(1.75))
                .text_color(gpui_kit::transparent_black())
                .child(suggestion),
        )
        .child(
            ui::keycap("Tab", cx)
                .relative()
                .top(text_size * GLYPHS_BELOW_MIDDLE),
        )
}

/// How solid the box is, letting a little of what lies behind it show through, blurred.
const COMPOSER_SOLID: f32 = 0.9;
const COMPOSER_BLUR: f32 = 16.;

/// How strongly the accent edges the box while YOLO is on.
const YOLO_EDGE: f32 = 0.5;

/// A one-line notice just above the box: a send refused, something waited on, or something
/// just done with how to take it back.
pub fn notice_line(notice: Notice, cx: &App) -> gpui_kit::Div {
    let t = Tokens::get(cx).clone();
    let line = div()
        .w_full()
        .max_w(super::column::composer(cx))
        .pb(SPACE_1H)
        .px(SPACE_2H)
        .flex()
        .items_center()
        .gap(SPACE_2)
        .text_size(crate::theme::ui_text(TEXT_XS));
    match notice {
        Notice::Refused(words, action) => {
            line.text_color(t.danger)
                .child(words)
                .children(action.map(|(label, act)| {
                    ui::sign_in::link("composer-notice-action", label, cx)
                        .clicks(move |_, window, cx| act(window, cx))
                }))
        }
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
                    .clicks(move |_, window, cx| act(window, cx)),
            ),
    }
}

/// Where the chat runs, quietly under the box: its host, project and branch, then its
/// account at the right.
fn context_line(strip: Strip, cx: &App) -> impl IntoElement {
    let mut t = Tokens::get(cx).clone();
    if strip.fresh {
        t.text_secondary = t.text_primary;
        t.text_tertiary = t.text_primary;
    }
    let name = |chevron: bool| {
        div()
            .h(px(24.))
            .px(SPACE_1H)
            .max_w(px(240.))
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded_full()
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_tertiary)
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
        Some(menu) => menu(
            BaseButton::new("composer-project")
                .accessibility_label(t!("Project"))
                .min_w_0()
                .rounded_full()
                .hover(move |style| style.bg(t.surface_hover))
                .tooltip(ui::tooltip(strip.path.clone()))
                .child(name(true)),
        )
        .into_any_element(),
        None => div()
            .id("composer-project")
            .min_w_0()
            .tooltip(ui::tooltip(strip.path.clone()))
            .child(name(false))
            .into_any_element(),
    };
    let host_name = |chevron: bool| {
        div()
            .h(px(24.))
            .min_w_0()
            .max_w(px(180.))
            .px(SPACE_1H)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded_full()
            .child(match strip.host {
                Some(_) => ui::icon(Icon::Server, 12.).flex_none().text_color(t.host),
                None => ui::icon(Icon::Laptop, 12.)
                    .flex_none()
                    .text_color(t.text_tertiary),
            })
            .child(
                ui::text(
                    strip.host.clone().unwrap_or_else(|| t!("This host").into()),
                    TEXT_XS,
                    FontWeight::NORMAL,
                )
                .min_w_0()
                .truncate()
                .text_color(match chevron {
                    true => t.text_secondary,
                    false => t.text_tertiary,
                }),
            )
            .when(chevron, |name| {
                name.child(
                    ui::icon(Icon::Chevron, 10.)
                        .with_transformation(Transformation::rotate(radians(FRAC_PI_2)))
                        .text_color(t.text_tertiary),
                )
            })
    };
    let host = match strip.hosts {
        Some(menu) => menu(
            BaseButton::new("composer-host")
                .accessibility_label(t!("Host"))
                .min_w_0()
                .rounded_full()
                .hover(move |style| style.bg(t.surface_hover))
                .child(host_name(true)),
        )
        .into_any_element(),
        None => host_name(false).into_any_element(),
    };
    let line = div()
        .w_full()
        .max_w(super::column::composer(cx))
        .min_w_0()
        .pt(SPACE_1H)
        .px(SPACE_2)
        .flex()
        .items_center()
        .gap(SPACE_1)
        .child(host)
        .child(slash(&t))
        .child(project)
        .children(strip.branch.is_some().then(|| slash(&t)))
        .children(strip.branch.map(|branch| {
            div()
                .min_w_0()
                .max_w(px(200.))
                .px(SPACE_1H)
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
        .children(strip.ports)
        .children(strip.pull)
        .child(div().flex_1())
        .children(
            strip
                .account
                .is_some()
                .then_some(strip.picked)
                .flatten()
                .map(|why| {
                    div()
                        .id("composer-smart-pick")
                        .flex_none()
                        .pr(SPACE_1H)
                        .child(ui::caps(t!("Smart pick"), cx))
                        .tooltip(ui::tooltip(why))
                }),
        )
        .children(
            strip
                .account
                .map(|account| div().min_w_0().flex().child(account)),
        );
    line.on_children_prepainted(|bounds, _, cx| {
        let strip = bounds.iter().map(|part| f32::from(part.size.width)).sum();
        Drawn::keep(cx, |drawn| Drawn { strip, ..drawn });
    })
}

/// The thin slash between the host, project and branch in the line under the box.
fn slash(t: &Tokens) -> impl IntoElement {
    ui::text("/", TEXT_SM, FontWeight::NORMAL).text_color(t.border_strong)
}

/// The widths the composer and what the context line under it holds were last drawn at, which
/// the shot example reports.
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
        .accessibility_label(t!("Stash"))
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
        .tooltip(ui::tooltip(format!("{} · &", t!("Stash"))))
        .on_click(move |_, window, cx| toggle(window, cx))
        .child(ui::icon(Icon::Stack, 14.).text_color(if open {
            t.text_primary
        } else {
            t.text_secondary
        }))
        .when(room == Room::Full, |button| button.child(t!("Stash")))
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
/// The widths are for a bar holding every control: Stash, the mode, the meter and the mic.
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
            w if w == 0. || w >= 550. => Room::Full,
            w if w >= 465. => Room::Compact,
            w if w >= 425. => Room::Tight,
            _ => Room::Bare,
        }
    }
}

fn snapshot_button(take: Act, cx: &App) -> IconButton {
    let keys = crate::keymap::hint_named(crate::snapshot::COMMAND, cx)
        .map_or_else(String::new, |keys| format!(" {keys}"));
    let label = match crate::snapshot::last_app() {
        Some(app) => format!("{}{keys}", t!("SnapShot {app}'s window", app = app)),
        None => format!(
            "{}{keys}",
            t!("SnapShot the window you were in before Sikemux")
        ),
    };
    IconButton::new("composer-snapshot", Icon::Camera, label)
        .glyph(16.)
        .width(31.)
        .on_click(move |_, window, cx| take(window, cx))
}

/// The room left for the bar's own controls at a composer `width` wide, and whether a control
/// at its end keeps its words; a control that has dropped them still takes its share of the bar.
/// `crowd` is how much more the bar holds than the full set `Room` is measured for.
fn shape(width: f32, aside: Option<AsideRoom>, crowd: f32) -> (Room, bool) {
    let labelled = width == 0. || aside.is_none_or(|aside| width >= aside.labelled_from);
    let taken = aside.filter(|_| !labelled).map_or(0., |aside| aside.bare);
    let room = if width == 0. {
        Room::Full
    } else {
        Room::of(width - taken - crowd)
    };
    (room, labelled)
}

/// What one control at its smallest takes from the bar, with the gap after it.
const CONTROL: f32 = 36.;

/// SAFE, AUTO or YOLO: whether the agent asks before it acts, has its own reviewer ask only
/// what is risky, or never asks, opening a menu of the modes it has. YOLO's letters run through
/// the rainbow the app paints it with.
fn mode_chip(modes: Modes, settable: bool, agent: &str, room: Room, cx: &App) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let mode = modes.current;
    let on = mode == PermissionMode::Bypass;
    let about = match mode {
        PermissionMode::Bypass => t!(
            "YOLO mode on — {agent} runs without approvals.",
            agent = agent
        ),
        PermissionMode::Auto => t!(
            "Auto mode — {agent}'s reviewer approves safe actions and asks you the rest.",
            agent = agent
        ),
        PermissionMode::WorkspaceWrite => {
            t!("Safe mode — {agent} asks before it acts.", agent = agent)
        }
    };
    let chip = BaseButton::new("composer-yolo")
        .accessibility_label(mode_word(mode))
        .selected(on)
        .disabled(!settable)
        .h(px(31.))
        .px(px(if room >= Room::Compact { 11. } else { 9.5 }))
        .flex()
        .items_center()
        .gap(px(5.))
        .rounded_full()
        .when(!settable, |chip| chip.opacity(0.42))
        .when(settable, |chip| {
            chip.cursor_pointer().hover(move |style| {
                style.bg(Hsla {
                    a: 0.07,
                    ..t.text_primary
                })
            })
        })
        .tooltip(ui::tooltip(about))
        .child(ui::icon(mode_icon(mode), 12.).text_color(match mode {
            PermissionMode::WorkspaceWrite => t.text_tertiary,
            _ => t.accent,
        }))
        .when(room >= Room::Compact, |chip| {
            chip.child(match on {
                true => ui::yolo::wordmark(10.5, &t).into_any_element(),
                false => ui::text(mode_word(mode), TEXT_3XS_H, FontWeight::SEMIBOLD)
                    .letter_spacing(0.08)
                    .text_color(match mode {
                        PermissionMode::Auto => t.text_secondary,
                        _ => t.text_tertiary,
                    })
                    .into_any_element(),
            })
        });
    if !settable {
        return chip.into_any_element();
    }
    let Modes { offered, pick, .. } = modes;
    ui::menu_button(
        "composer-mode",
        Anchor::BottomLeft,
        chip,
        move |mut popup, _, cx| {
            let yolo_keys = crate::keymap::hint(actions::ToggleYolo::ID, cx);
            for &choice in &offered {
                let pick = pick.clone();
                let t = Tokens::get(cx).clone();
                popup = popup.item(
                    ui::MenuRow::new(mode_label(choice))
                        .icon(mode_icon(choice))
                        .tiled()
                        .about(mode_about(choice))
                        .when(choice == PermissionMode::Bypass, |row| {
                            row.title(move || ui::yolo::wordmark(11., &t))
                        })
                        .when_some(
                            yolo_keys
                                .clone()
                                .filter(|_| choice == PermissionMode::Bypass),
                            |row, keys| row.hint(keys),
                        )
                        .chosen(choice == mode)
                        .item(cx)
                        .on_click(move |_, window, cx| pick(choice, window, cx)),
                );
            }
            popup
        },
    )
    .into_any_element()
}

fn mode_word(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Bypass => "YOLO",
        PermissionMode::Auto => t!("AUTO"),
        PermissionMode::WorkspaceWrite => t!("SAFE"),
    }
}

fn mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Bypass => "YOLO",
        PermissionMode::Auto => t!("Auto"),
        PermissionMode::WorkspaceWrite => t!("Safe"),
    }
}

fn mode_about(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Bypass => t!("Never asks"),
        PermissionMode::Auto => t!("Asks only what's risky"),
        PermissionMode::WorkspaceWrite => t!("Asks before it acts"),
    }
}

fn mode_icon(mode: PermissionMode) -> Icon {
    match mode {
        PermissionMode::Bypass => Icon::BoltFill,
        PermissionMode::Auto => Icon::ShieldBolt,
        PermissionMode::WorkspaceWrite => Icon::Lock,
    }
}

/// What a choice is called, as its picker's trigger shows it.
pub(crate) fn chosen(picker: &ConfigPicker, name: &str) -> String {
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
    face: Face,
    name: &'static str,
    enabled: bool,
    room: Room,
    cx: &App,
) -> BaseButton {
    let t = Tokens::get(cx).clone();
    let trigger = BaseButton::new(SharedString::from(format!("picker-{id}")))
        .accessibility_label(name)
        .disabled(!enabled)
        .when(enabled, |trigger| {
            trigger.cursor_pointer().hover(move |style| {
                style.bg(Hsla {
                    a: 0.07,
                    ..t.text_primary
                })
            })
        });
    picker_face(trigger, face, enabled, room, cx)
}

/// What a picker's trigger shows: the choice, the agent's mark on the first, and the effort
/// on the model's.
struct Face<'a> {
    label: String,
    provider: Option<&'a str>,
    effort: Option<String>,
}

/// How a picker's trigger looks, on whichever button opens it: the agent's mark when it is
/// the first, what is chosen, the effort after its gauge when it has one, and a chevron.
fn picker_face<E: Styled + ParentElement + IntoElement>(
    trigger: E,
    Face {
        label,
        provider,
        effort,
    }: Face,
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
        .max_w(px(if effort.is_some() { 300. } else { 220. }))
        .when(first, |trigger| trigger.min_w_0())
        .when(!first, |trigger| trigger.flex_none())
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .rounded_full()
        .text_size(crate::theme::ui_text(TEXT_2XS))
        .font_weight(FontWeight::MEDIUM)
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
        .when_some(effort.filter(|_| room > Room::Bare), |trigger, effort| {
            trigger
                .child(div().flex_none().text_color(t.text_tertiary).child("·"))
                .child(
                    ui::icon(Icon::Gauge, 13.)
                        .flex_none()
                        .text_color(t.text_tertiary),
                )
                .when(room == Room::Full, |trigger| {
                    trigger.child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_color(t.text_tertiary)
                            .child(effort),
                    )
                })
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
        let face = Face {
            label,
            provider,
            effort: None,
        };
        return picker_trigger(&picker.id, face, name, enabled, room, cx).into_any_element();
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
    let face = Face {
        label,
        provider,
        effort: None,
    };
    picker_face(trigger, face, enabled, room, cx)
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
                            .when(choice.is_default, |row| row.hint(t!("Default")))
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
    expired: Option<(ExpiredCache, Summarise)>,
    provider: &str,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let percent = if usage.limit() > 0. {
        (usage.used / usage.limit() * 100.).clamp(0., 100.)
    } else {
        0.
    };
    let signal = match percent {
        p if p >= 90. => t.danger,
        p if p >= 70. => t.warn,
        _ if sikemux_agents::agent_catalog::agent(provider).is_some_and(|agent| agent.usage) => {
            theme::brand(provider)
        }
        _ => t.accent,
    };
    let percent_ink = if percent >= 70. {
        signal
    } else {
        t.text_primary
    };
    let summarising = expired.as_ref().filter(|(expired, _)| expired.summarise);
    let ring_ink = if summarising.is_some() {
        t.accent
    } else {
        signal
    };
    let count = summarising.map(|(expired, _)| expired.tokens);
    let compactions_shown = compactions.as_ref().map(|compactions| compactions.count);
    let card = Rc::new({
        let expired = expired.clone();
        move |interactive: bool, cx: &App| {
            context_card(
                usage.clone(),
                compactions.clone(),
                expired.as_ref().map(|(expired, summarise)| {
                    (expired.clone(), interactive.then(|| summarise.clone()))
                }),
                percent,
                signal,
                percent_ink,
                cx,
            )
        }
    });
    let width = if expired.is_some() { 320. } else { 236. };
    let tip = card.clone();
    let meter = move |open: bool| {
        let hover = Hsla {
            a: 0.07,
            ..t.text_primary
        };
        let tip = tip.clone();
        let ring = match compactions_shown.as_ref() {
            None => ui::ring(Some((percent, ring_ink)), 14., 1.9, t.line).into_any_element(),
            Some(count) => counted_ring(percent, ring_ink, *count, &t),
        };
        let count = count.map(|tokens| {
            ui::text(
                crate::ui::numbers::compact(tokens as f64),
                TEXT_2XS_H,
                FontWeight::MEDIUM,
            )
            .font_features(theme::tabular())
            .text_color(t.accent)
        });
        div()
            .id("context-meter")
            .h(px(31.))
            .min_w(px(31.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .gap(SPACE_1H)
            .px(if count.is_some() { SPACE_2 } else { px(0.) })
            .rounded_full()
            .when(open, |meter| meter.bg(hover))
            .hover(move |style| style.bg(hover))
            .when(!open, |meter| {
                meter.tooltip(ui::card_tooltip(width, move |cx| tip(false, cx)))
            })
            .child(ring)
            .children(count)
    };
    if expired.is_none() {
        return meter(false).into_any_element();
    }
    let t = Tokens::get(cx).clone();
    Popover::new("context-meter-card")
        .anchor(Anchor::BottomLeft)
        .offset(SPACE_1H)
        .trigger_with(move |open, _, _| meter(open).cursor_pointer().into_any_element())
        .content(move |_, _, cx| {
            div()
                .w(px(width))
                .px(SPACE_3)
                .pt(SPACE_2H)
                .pb(px(11.))
                .rounded(RADIUS_3)
                .bg(t.surface_overlay)
                .border_1()
                .border_color(t.border)
                .shadow(t.shadow_float())
                .text_color(t.text_primary)
                .child(card(true, cx))
                .into_any_element()
        })
        .into_any_element()
}

/// The context ring grown to hold how often the conversation was compacted.
fn counted_ring(percent: f64, signal: Hsla, count: u32, t: &Tokens) -> AnyElement {
    // Odd so the ring and a figure, whose height is odd at these sizes, share a pixel centre.
    const DIAMETER: f32 = 19.;
    let size = if count < 10 { 10. } else { 8.5 };
    div()
        .relative()
        .size(px(DIAMETER))
        .child(div().absolute().inset_0().child(ui::ring(
            Some((percent, signal)),
            DIAMETER,
            2.,
            t.line,
        )))
        .child(
            ui::text(count.to_string(), size, FontWeight::SEMIBOLD)
                .w_full()
                .line_height(px(DIAMETER))
                .text_center()
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
    expired: Option<(ExpiredCache, Option<Summarise>)>,
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
                crate::ui::numbers::compact(usage.used),
                TEXT_2XS_H,
                FontWeight::NORMAL,
            )
            .text_color(t.text_primary),
        )
        .child(
            ui::text(
                format!("/ {}", crate::ui::numbers::compact(usage.size)),
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
                    ui::text(t!("Context window"), TEXT_XS, FontWeight::NORMAL)
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
                .child(row(t!("Tokens"), tokens.into_any_element()))
                .children(usage.compact_at.map(|at| {
                    let at = ui::text(
                        crate::ui::numbers::compact(at),
                        TEXT_2XS_H,
                        FontWeight::NORMAL,
                    )
                    .font_features(theme::tabular())
                    .text_color(t.text_primary);
                    row(t!("Compacts at"), at.into_any_element())
                }))
                .children(compactions.map(|compactions| {
                    let ink = match compactions.count {
                        10.. => t.danger,
                        5.. => t.warn,
                        _ => t.text_primary,
                    };
                    let times = match compactions.count {
                        1 => t!("Once").to_owned(),
                        count => t!("{count} times", count = count),
                    };
                    row(
                        t!("Compacted"),
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
                        t!("Session cost"),
                        ui::text(
                            crate::ui::numbers::money(cost.amount),
                            TEXT_2XS_H,
                            FontWeight::NORMAL,
                        )
                        .font_features(theme::tabular())
                        .text_color(t.text_primary)
                        .into_any_element(),
                    )
                })),
        )
        .children(expired.map(|(expired, summarise)| expired_section(expired, summarise, cx)))
        .into_any_element()
}

/// Under the meter's numbers, once the prompt cache has expired: what the next message costs,
/// and the switch that summarises the chat before it. Hovered, the switch only shows.
fn expired_section(expired: ExpiredCache, summarise: Option<Summarise>, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let tokens = crate::ui::numbers::compact(expired.tokens as f64);
    let switch = Switch::new("context-summarise")
        .checked(expired.summarise)
        .when_some(summarise, |switch, summarise| {
            switch.on_change(move |on: &bool, window: &mut Window, cx: &mut App| {
                summarise(*on, window, cx)
            })
        });
    div()
        .flex()
        .flex_col()
        .child(div().mx(-SPACE_3).my(SPACE_3).h(px(1.)).bg(t.border))
        .child(
            ui::text(
                t!(
                    "Cache expired {ago} · your next message re-reads all {tokens} tokens",
                    ago = ui::time::ago(expired.at),
                    tokens = tokens
                ),
                TEXT_2XS_H,
                FontWeight::NORMAL,
            )
            .whitespace_normal()
            .line_height(px(16.))
            .text_color(t.text_secondary),
        )
        .child(
            div()
                .mt(SPACE_2H)
                .flex()
                .items_center()
                .justify_between()
                .gap(SPACE_3)
                .child(
                    ui::text(
                        t!("Summarise the chat before your next message"),
                        TEXT_2XS_H,
                        FontWeight::NORMAL,
                    )
                    .text_color(t.text_primary),
                )
                .child(div().flex_none().child(switch)),
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
            t!("Steers into this turn — ⌘↵ queues behind it")
        } else {
            t!("Queues behind this turn — ⌘↵ steers into it")
        }
    });
    BaseButton::new("composer-send")
        .accessibility_label(if stopping {
            t!("Stop agent")
        } else {
            t!("Send message")
        })
        .disabled(!stopping && !drafted)
        .ml(px(3.))
        .size(px(30.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_2)
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
