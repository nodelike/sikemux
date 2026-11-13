use crate::battery::battery_chip;
use crate::core::Target;
use crate::tabs;
use crate::tasks::chip::run_chip;
use crate::workbench::actions;
use sikemux_core::server::call;
use sikemux_core::server::layout::TabParams;
use sikemux_core::server::layout::TabsParams;
use sikemux_view::layout::At;
use sikemux_view::layout::OpenTab;
use sikemux_view::layout::ProjectLayout;
use std::path::Path;
use std::time::Duration;

use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
use gpui_kit::component::WindowExt;

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, App, ClickEvent, ClipboardItem, Context, FontWeight, InteractiveElement,
    IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, WeakEntity,
    Window, WindowControlArea,
};
use sikemux_view::layout::{Pane, PaneKind, Tab as LayoutTab};
use sikemux_wire::pty::shell_protocol::SshLink;

use crate::agents::menu::AgentMenu;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_2H, SPACE_4, TEXT_2XS, TEXT_3XS, TEXT_XS,
};
use crate::toast;
use crate::ui::{self, IconButton, Tab};
use crate::workbench::{Strip, Workbench};

/// The bar across the window's top, under the window buttons.
const BAR_HEIGHT: f32 = 36.;
/// The tabs' row under the bar, with the gutter above it and a little less below.
const TAB_ROW_HEIGHT: f32 = 42.;

/// How wide the window is, in points, before the bar's chips keep their words; narrower,
/// they keep only their marks so they stay clear of the place in the middle.
const ROOMY_BAR: f32 = 1360.;

thread_local! {
    /// How wide the chips at the bar's right end were last drawn, which the place in the middle
    /// keeps clear of.
    static CHIPS_WIDTH: std::cell::Cell<f32> = const { std::cell::Cell::new(0.) };
}

/// How tall the bar and, when a place has tabs, the row of them under it stand.
pub fn top_height(bench: &Workbench) -> f32 {
    BAR_HEIGHT
        + if strip_with_tabs(bench).is_some() {
            TAB_ROW_HEIGHT
        } else {
            0.
        }
}

/// The strip in front while it holds a tab: a place with none shows no row at all.
fn strip_with_tabs(bench: &Workbench) -> Option<Strip> {
    bench.strip().filter(|strip| !bench.tabs(strip).is_empty())
}

/// The bar, holding the window buttons, where the window is and the chips, then the
/// project's tabs on a row of their own from the window's left edge to its right.
pub fn top_bar(
    bench: &Workbench,
    window: &Window,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let roomy = window.viewport_size().width >= px(ROOMY_BAR);
    let t = Tokens::get(cx).clone();
    let tabs = strip_with_tabs(bench).map(|strip| strip_tabs(bench, &strip, cx));
    let bar = div()
        .on_children_prepainted(|bounds, window, cx| {
            let chips = bounds
                .last()
                .map_or(0., |chips| f32::from(chips.size.width));
            if (CHIPS_WIDTH.get() - chips).abs() > 0.5 {
                CHIPS_WIDTH.set(chips);
                ui::redraw_after_paint(window, cx);
            }
        })
        .id("top-bar")
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .h(px(BAR_HEIGHT))
        .when(!crate::see_through::clear(cx), |bar| bar.bg(t.surface))
        .border_b_1()
        .border_color(t.border)
        .window_control_area(WindowControlArea::Drag)
        .when(cfg!(target_os = "macos"), |bar| {
            bar.child(div().w(px(80.)).flex_none())
        })
        .child(div().flex_1())
        .child(crate::hosts::top_place::view(
            bench,
            px(CHIPS_WIDTH.get()) + SPACE_2,
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(SPACE_2)
                .pr(SPACE_4)
                .children(
                    crate::overload::chip(bench, roomy, cx)
                        .map(|chip| [chip, separator(cx)])
                        .into_iter()
                        .flatten(),
                )
                .children(
                    git_chip(bench, cx)
                        .map(|chip| {
                            crate::settings::leaves(div().child(chip), cx).into_any_element()
                        })
                        .map(|chip| [chip, separator(cx)])
                        .into_iter()
                        .flatten(),
                )
                .children(
                    run_chip(bench, cx)
                        .map(|chip| [chip, separator(cx)])
                        .into_iter()
                        .flatten(),
                )
                .children(battery_chip(bench, cx))
                .child(clock(cx))
                .child(
                    div()
                        .flex()
                        .gap(SPACE_0)
                        .children(crate::agents::limits::chip(bench, cx))
                        .child(
                            IconButton::new(
                                "focus",
                                Icon::Focus,
                                match crate::keymap::hint(actions::FocusMode::ID, cx) {
                                    Some(keys) => {
                                        t!("Focus mode — hide rails — {keys}", keys = keys)
                                    }
                                    None => t!("Focus mode — hide rails").to_owned(),
                                },
                            )
                            .width(30.)
                            .pressed(!bench.rails.side_open && !bench.rails.tab_open)
                            .on_click(|_, window, cx| {
                                crate::keymap::dispatch(actions::FocusMode::ID, window, cx)
                            }),
                        ),
                ),
        );
    let row = tabs.map(|(copy, tabs)| {
        let glide = bench.strip.glide.borrow();
        div()
            .on_children_prepainted(glide.keep_origin())
            .id("tab-row")
            .h(px(TAB_ROW_HEIGHT))
            .flex()
            .flex_none()
            .items_center()
            .pt(SPACE_2H)
            .pb(SPACE_1H)
            .px(SPACE_2H)
            .window_control_area(WindowControlArea::Drag)
            .child(
                div()
                    .h_full()
                    .min_w_0()
                    .overflow_hidden()
                    .relative()
                    .flex()
                    .children(copy)
                    .child(
                        crate::settings::leaves(div(), cx)
                            .on_children_prepainted(glide.keep_items(0))
                            .id("tab-strip")
                            .h_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(SPACE_1)
                            .overflow_x_scroll()
                            .track_scroll(&bench.strip.scroll)
                            .children(tabs),
                    ),
            )
            .child(
                IconButton::new("new-tab", Icon::Plus, t!("New tab"))
                    .glyph(13.)
                    .on_click(|_, window, cx| {
                        crate::keymap::dispatch(actions::NewTab::ID, window, cx)
                    }),
            )
    });
    div().flex().flex_col().w_full().child(bar).children(row)
}

fn separator(cx: &App) -> AnyElement {
    div()
        .w(px(1.))
        .h(px(16.))
        .bg(Tokens::get(cx).line)
        .into_any_element()
}

/// Where the project's repository stands: its branch, and how far it is from its upstream.
fn git_chip(bench: &Workbench, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let project = bench.project()?;
    let git = project.git.as_ref()?;
    let host = project.code_host.as_ref();
    let mark = match host {
        Some(host) if let Some(mark) = Icon::named(&host.host) => {
            ui::picture(mark, 12., t.text_primary)
        }
        _ => ui::picture(Icon::Git, 12., if git.dirty { t.warn } else { t.live }),
    };
    let mut about: Vec<String> = host
        .map(|host| vec![host.label.clone(), host.repo.clone()])
        .unwrap_or_default();
    about.push(git.branch.clone());
    about.push(if git.dirty {
        t!("changed").into()
    } else {
        t!("clean").into()
    });
    about.extend((git.ahead > 0).then(|| t!("ahead {count}", count = git.ahead)));
    about.extend((git.behind > 0).then(|| t!("behind {count}", count = git.behind)));
    let count = |arrow: &str, count: u32, color| {
        (count > 0).then(|| {
            ui::text(format!("{arrow}{count}"), TEXT_3XS, FontWeight::NORMAL).text_color(color)
        })
    };
    Some(
        div()
            .id("git-chip")
            .h(px(24.))
            .max_w(px(200.))
            .px(SPACE_1)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded(crate::theme::RADIUS_1)
            .font_family(crate::theme::MONO)
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(t.text_tertiary)
            .hover(move |style| style.text_color(t.text_primary))
            .tooltip(ui::tooltip(about.join(" · ")))
            .on_click(|_, window, cx| crate::keymap::dispatch(actions::FocusGit::ID, window, cx))
            .child(mark)
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .text_color(t.text_primary)
                    .child(git.branch.clone()),
            )
            .when(git.ahead > 0 || git.behind > 0, |chip| {
                chip.child(
                    div()
                        .flex()
                        .gap(SPACE_1)
                        .children(count("↑", git.ahead, t.live))
                        .children(count("↓", git.behind, t.warn)),
                )
            })
            .into_any_element(),
    )
}

/// What the strip remembers between draws to move its tabs: those drawn last, for telling
/// those that have just opened and closed, where they were, for the chosen surface to glide
/// between them, and how far they are scrolled, with the tab last scrolled into view.
#[derive(Default)]
pub struct StripMotion {
    pub(crate) seen: std::cell::RefCell<motion::ListMemory<LayoutTab>>,
    pub(crate) glide: std::cell::RefCell<motion::GlideTrack>,
    pub(crate) scroll: gpui_kit::ScrollHandle,
    pub(crate) scrolled_to: std::cell::RefCell<Option<String>>,
}

/// The strip's tabs: one just opened widens into its place, and one just closed narrows away,
/// unless the strip changed most of its tabs at once, as another project's does. The chosen
/// surface glides to the tab brought to the front; that copy comes first.
fn strip_tabs(
    bench: &Workbench,
    strip: &Strip,
    cx: &mut Context<Workbench>,
) -> (Option<AnyElement>, Vec<AnyElement>) {
    let front = bench.tab().map(|tab| tab.id.clone());
    let listed = bench.tabs(strip);
    let shown = listed
        .iter()
        .map(|tab| (tab.id.clone(), tab.clone()))
        .collect();
    let lasting = motion::TAB_ARRIVAL
        .grow
        .duration
        .max(motion::TAB_DEPARTURE.close.duration);
    let mut seen = bench.strip.seen.borrow_mut();
    let (arriving, left) = seen.update(format!("{strip:?}"), shown, 2, lasting);
    let mut order: Vec<String> = listed.iter().map(|tab| tab.id.clone()).collect();
    for gone in left {
        order.insert(gone.at.min(order.len()), gone.id.clone());
    }
    let mut scrolled_to = bench.strip.scrolled_to.borrow_mut();
    if *scrolled_to != front {
        if let Some(at) = order.iter().position(|id| Some(id) == front.as_ref()) {
            bench.strip.scroll.scroll_to_item(at);
        }
        *scrolled_to = front.clone();
    }
    let mut glide = bench.strip.glide.borrow_mut();
    let glided = glide.choose(front.clone(), cx.reduce_motion());
    glide.set_order(order);
    let chosen = Tokens::get(cx).surface_chosen;
    let copy = glide.copy(move |copy| copy.rounded(crate::theme::RADIUS_2).bg(chosen));
    let mut drawn: Vec<AnyElement> = listed
        .iter()
        .map(|tab| {
            let active = front.as_deref() == Some(tab.id.as_str());
            let drawn = strip_tab(bench, strip, tab, active, glided && active, true, cx);
            match arriving.contains(&tab.id) {
                true => {
                    let key = SharedString::from(format!("tab-arrive:{}", tab.id));
                    motion::arriving(key, motion::TAB_ARRIVAL, drawn).into_any_element()
                }
                false => drawn,
            }
        })
        .collect();
    for gone in left {
        let tab = strip_tab(bench, strip, &gone.item, false, false, false, cx);
        let key = SharedString::from(format!("tab-depart:{}", gone.id));
        let tab = motion::departing(key, gone.since, motion::TAB_DEPARTURE, tab);
        drawn.insert(gone.at.min(drawn.len()), tab.into_any_element());
    }
    (copy, drawn)
}

/// A tab of the strip; one that is `live` takes clicks, one drawn only while it closes does not.
fn strip_tab(
    bench: &Workbench,
    strip: &Strip,
    tab: &LayoutTab,
    active: bool,
    glided: bool,
    live: bool,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let tab_id = tab.id.clone();
    let mut base = Tab::new(
        SharedString::from(format!("tab-{}", tab.id)),
        label(bench, tab),
    )
    .active(active)
    .glided(glided)
    .italic(tab.preview);
    if live {
        base = base
            .on_click(cx.listener({
                let (strip, tab) = (strip.clone(), tab_id.clone());
                move |bench, event: &ClickEvent, _, _| match event.click_count() {
                    2 => bench.keep_tab(&strip, &tab),
                    _ => bench.activate_tab(&strip, &tab),
                }
            }))
            .on_close(cx.listener({
                let strip = strip.clone();
                move |bench, _, _, cx| {
                    bench.close_tabs_asking(strip.clone(), vec![tab_id.clone()], cx)
                }
            }));
    }
    let view = first_view(&tab.pane);
    let shown = match view {
        Some((PaneKind::Agent, target)) => match match strip {
            Strip::Project(project) => bench.agents_of(project),
            Strip::Host(_) | Strip::Terminals => Vec::new(),
        }
        .into_iter()
        .find(|agent| &agent.id == target)
        {
            Some(agent) => base
                .mark(agent.mark(19., cx))
                .note(
                    agent
                        .handle()
                        .map(|name| SharedString::from(format!("@{name}"))),
                )
                .counts(crate::agents::counts(&agent, cx))
                .tail(ui::turn_mark(&agent.id, agent.turn.as_ref())),
            None => base,
        },
        Some((kind, target)) => {
            let unsaved = kind == PaneKind::Files
                && bench
                    .editors
                    .read(cx)
                    .is_dirty(&crate::editor::FileKey::new(bench.place(), target));
            let file_icon = (kind == PaneKind::Files)
                .then(|| Path::new(target).file_name())
                .flatten()
                .and_then(|name| crate::file_icons::icon(&name.to_string_lossy(), cx));
            base.mark(match file_icon {
                Some(icon) => gpui_kit::img(icon).size(px(14.)).into_any_element(),
                None => ui::picture(
                    crate::pane_kind::of(kind).icon,
                    13.,
                    crate::pane_kind::tint(kind, &t),
                )
                .into_any_element(),
            })
            .tail(match ssh_link(bench, kind, target) {
                Some(link) => Some(ui::dot(link, cx).into_any_element()),
                None if unsaved => Some(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .bg(t.warn)
                        .into_any_element(),
                ),
                None if kind == PaneKind::Files => bench
                    .files
                    .letter(Path::new(target), cx)
                    .map(IntoElement::into_any_element),
                None => None,
            })
        }
        None => base,
    };
    if !live {
        return div().flex().min_w_0().child(shown).into_any_element();
    }
    let menu = tab_menu(bench, strip, tab, cx);
    let drawn = div()
        .id(SharedString::from(format!("tab-menu-{}", tab.id)))
        .flex()
        .min_w_0()
        .child(shown)
        .context_menu(ui::menu(menu))
        .into_any_element();
    let index = bench
        .tabs(strip)
        .iter()
        .position(|other| other.id == tab.id)
        .unwrap_or(0);
    let dragged = crate::drag::DraggedTab {
        strip: strip.clone(),
        tab: tab.id.clone(),
        label: label(bench, tab),
    };
    let reorder = bench
        .reorder
        .as_ref()
        .filter(|reorder| reorder.strip == *strip && cx.has_active_drag());
    crate::drag::tab(drawn, dragged, index, reorder, &cx.entity()).into_any_element()
}

/// How an SSH terminal's connection stands, for its tab's dot.
fn ssh_link(bench: &Workbench, kind: PaneKind, target: &str) -> Option<ui::Tone> {
    let session = bench
        .session(target)
        .filter(|_| kind == PaneKind::Terminal)?;
    session.ssh_alias.as_ref()?;
    Some(match session.ssh {
        Some(SshLink::Connected) => ui::Tone::Ok,
        Some(SshLink::Connecting) => ui::Tone::Pending,
        Some(SshLink::Disconnected) => ui::Tone::Failed,
        None => ui::Tone::Idle,
    })
}

/// What right-clicking a tab offers, wherever it is listed: an agent's menu for an
/// agent's tab, else the tab's own.
pub fn tab_menu(
    bench: &Workbench,
    strip: &Strip,
    tab: &LayoutTab,
    cx: &Context<Workbench>,
) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    let tab_menu = TabMenu::new(bench, strip, tab, cx);
    let agent_menu = match first_view(&tab.pane) {
        Some((PaneKind::Agent, agent)) => AgentMenu::new(bench, agent, cx),
        _ => None,
    };
    let separable = bench.separable(tab, cx);
    let (bench, strip, tab) = (cx.entity().downgrade(), strip.clone(), tab.id.clone());
    move |popup, window, cx| {
        let popup = match &agent_menu {
            Some(menu) => menu.build(popup, window, cx),
            None => tab_menu.build(popup),
        };
        let Some((view, kind)) = separable.clone() else {
            return popup;
        };
        let (strip, tab) = (strip.clone(), tab.clone());
        popup.separator().item(
            PopupMenuItem::new(if kind == PaneKind::Files {
                t!("Move Focused File Back to Editor")
            } else {
                t!("Move Focused Pane Back to Tab Bar")
            })
            .on_click(on_bench(&bench, move |bench, _| {
                bench.separate_pane(&strip, &tab, &view)
            })),
        )
    }
}

/// What right-clicking a strip tab offers, as today's tab menu does.
struct TabMenu {
    bench: WeakEntity<Workbench>,
    strip: Strip,
    tab: String,
    kind: Option<PaneKind>,
    /// The file a Files tab shows, and that path inside its project.
    file: Option<(String, String)>,
    named: bool,
    label: SharedString,
    /// The other tabs Close Others closes, which leaves agents' tabs open.
    others: Vec<String>,
    /// The strip's file tabs in order, each with whether it is saved, for a file tab's closes.
    files: Vec<(String, bool)>,
}

impl TabMenu {
    fn new(bench: &Workbench, strip: &Strip, tab: &LayoutTab, cx: &Context<Workbench>) -> Self {
        let view = first_view(&tab.pane);
        let project = match strip {
            Strip::Project(id) => bench
                .projects()
                .iter()
                .find(|project| &project.id == id)
                .map(|project| project.path.clone()),
            Strip::Host(_) | Strip::Terminals => None,
        };
        let file = view
            .filter(|(kind, _)| *kind == PaneKind::Files)
            .map(|(_, path)| {
                let relative = project
                    .as_deref()
                    .and_then(|project| Path::new(path).strip_prefix(project).ok())
                    .map_or_else(
                        || path.clone(),
                        |inside| inside.to_string_lossy().into_owned(),
                    );
                (path.clone(), relative)
            });
        Self {
            bench: cx.entity().downgrade(),
            strip: strip.clone(),
            tab: tab.id.clone(),
            kind: view.map(|(kind, _)| kind),
            file,
            named: tab.title.is_some(),
            label: label(bench, tab),
            others: bench
                .tabs(strip)
                .iter()
                .filter(|other| other.id != tab.id)
                .filter(|other| !matches!(first_view(&other.pane), Some((PaneKind::Agent, _))))
                .map(|other| other.id.clone())
                .collect(),
            files: bench
                .tabs(strip)
                .iter()
                .filter_map(|other| match first_view(&other.pane)? {
                    (PaneKind::Files, path) => Some((
                        other.id.clone(),
                        !bench
                            .editors
                            .read(cx)
                            .is_dirty(&crate::editor::FileKey::new(bench.place(), path)),
                    )),
                    _ => None,
                })
                .collect(),
        }
    }

    fn build(&self, mut popup: PopupMenu) -> PopupMenu {
        let (strip, tab) = (self.strip.clone(), self.tab.clone());
        if self.kind == Some(PaneKind::Terminal) {
            let (strip, tab, label) = (strip.clone(), tab.clone(), self.label.to_string());
            popup = popup.item(
                PopupMenuItem::new(t!("Rename…"))
                    .icon(Icon::Pencil)
                    .on_click(on_bench(&self.bench, move |bench, cx| {
                        let (strip, tab) = (strip.clone(), tab.clone());
                        bench.prompt(
                            t!("Rename tab"),
                            label.clone(),
                            move |bench, name, _, _| {
                                if !name.is_empty() {
                                    bench.rename_tab(&strip, &tab, &name);
                                }
                            },
                            cx,
                        );
                    })),
            );
        }
        if self.kind == Some(PaneKind::Terminal) && self.named {
            let (strip, tab) = (strip.clone(), tab.clone());
            popup = popup.item(
                PopupMenuItem::new(t!("Use Automatic Name"))
                    .icon(Icon::Wand)
                    .on_click(on_bench(&self.bench, move |bench, _| {
                        bench.rename_tab(&strip, &tab, "")
                    })),
            );
        }
        if self.kind == Some(PaneKind::Terminal) {
            popup = popup.separator();
        }
        if self.file.is_some() {
            return self.file_closes(popup);
        }
        if self.kind != Some(PaneKind::Agent) {
            let (strip, tab) = (strip.clone(), tab.clone());
            popup = popup.item(
                PopupMenuItem::new(t!("Duplicate"))
                    .icon(Icon::SquarePlus)
                    .on_click(on_bench(&self.bench, move |bench, _| {
                        bench.duplicate_tab(&strip, &tab)
                    })),
            );
        }
        let close = {
            let (strip, tab) = (strip.clone(), tab);
            on_bench(&self.bench, move |bench, cx| {
                bench.close_tabs_asking(strip.clone(), vec![tab.clone()], cx)
            })
        };
        let others = self.others.clone();
        popup = popup
            .item(
                PopupMenuItem::new(t!("Close"))
                    .icon(Icon::Close)
                    .action(Box::new(crate::workbench::actions::CloseTab))
                    .on_click(close),
            )
            .item(
                PopupMenuItem::new(t!("Close Others"))
                    .icon(Icon::Close)
                    .disabled(others.is_empty())
                    .on_click(on_bench(&self.bench, move |bench, cx| {
                        bench.close_tabs_asking(strip.clone(), others.clone(), cx)
                    })),
            );
        popup
    }

    /// A file tab's closes and paths, as the Mac's file tab menu has them.
    fn file_closes(&self, popup: PopupMenu) -> PopupMenu {
        let strip = self.strip.clone();
        let at = self.files.iter().position(|(id, _)| *id == self.tab);
        let ids = |files: &[(String, bool)]| files.iter().map(|(id, _)| id.clone()).collect();
        let left: Vec<String> = at.map_or(Vec::new(), |at| ids(&self.files[..at]));
        let right: Vec<String> = at.map_or(Vec::new(), |at| ids(&self.files[at + 1..]));
        let others: Vec<String> = ids(&self.files)
            .into_iter()
            .filter(|id: &String| *id != self.tab)
            .collect();
        let saved: Vec<String> = self
            .files
            .iter()
            .filter(|(_, saved)| *saved)
            .map(|(id, _)| id.clone())
            .collect();
        let all: Vec<String> = ids(&self.files);
        let close = |label: &'static str, tabs: Vec<String>, always: bool| {
            let strip = strip.clone();
            PopupMenuItem::new(label)
                .icon(Icon::Close)
                .disabled(!always && tabs.is_empty())
                .on_click(on_bench(&self.bench, move |bench, cx| {
                    bench.close_tabs_asking(strip.clone(), tabs.clone(), cx)
                }))
        };
        let mut popup = popup
            .item(
                close(t!("Close"), vec![self.tab.clone()], true)
                    .action(Box::new(crate::workbench::actions::CloseTab)),
            )
            .item(close(t!("Close Others"), others, false))
            .item(close(t!("Close to the Left"), left, false))
            .item(close(t!("Close to the Right"), right, false))
            .item(close(t!("Close Saved"), saved, false))
            .item(close(t!("Close All"), all, true));
        if let Some((path, relative)) = &self.file {
            let (copied, relative, revealed) = (path.clone(), relative.clone(), path.clone());
            popup = popup
                .separator()
                .item(
                    PopupMenuItem::new(t!("Copy Path"))
                        .icon(Icon::Copy)
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                            window.push_notification(toast::success(t!("Copied path")), cx);
                        }),
                )
                .item(
                    PopupMenuItem::new(t!("Copy Relative Path"))
                        .icon(Icon::Copy)
                        .on_click(move |_, window, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(relative.clone()));
                            window
                                .push_notification(toast::success(t!("Copied relative path")), cx);
                        }),
                )
                .separator()
                .item(
                    PopupMenuItem::new(crate::i18n::tr(ui::REVEAL))
                        .icon(Icon::Folder)
                        .on_click(move |_, _, cx| cx.reveal_path(Path::new(&revealed))),
                );
        }
        popup
    }
}

/// A menu item's click, doing `work` on the store.
pub(crate) fn on_bench(
    bench: &WeakEntity<Workbench>,
    work: impl Fn(&mut Workbench, &mut Context<Workbench>) + 'static,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let bench = bench.clone();
    move |_, _, cx| {
        let _ = bench.update(cx, |bench, cx| work(bench, cx));
    }
}

/// A tab's name: the person's, else what its first view shows.
pub fn label(bench: &Workbench, tab: &LayoutTab) -> SharedString {
    match &tab.title {
        Some(title) => title.clone().into(),
        None => view_label(bench, &tab.pane),
    }
}

/// What a pane is called by the first view in it: its agent's task, its shell's title or its kind.
pub fn view_label(bench: &Workbench, pane: &Pane) -> SharedString {
    match first_view(pane) {
        Some((PaneKind::Agent, target)) => bench
            .agent_by_id(target)
            .map_or_else(|| t!("Agent").into(), |agent| agent.title.into()),
        Some((PaneKind::Terminal, target)) => bench
            .session(target)
            .and_then(|session| {
                session
                    .window_title
                    .clone()
                    .or_else(|| session.title.clone())
            })
            .map_or_else(|| t!("Terminal").into(), SharedString::from),
        Some((PaneKind::Files, target)) => Path::new(target).file_name().map_or_else(
            || t!("Files").into(),
            |name| SharedString::from(name.to_string_lossy().into_owned()),
        ),
        Some((kind, _)) => crate::pane_kind::of(kind).label.into(),
        None => "".into(),
    }
}

/// The kind of a pane's first view and what it shows, which name the tab holding it.
pub fn first_view(pane: &Pane) -> Option<(PaneKind, &String)> {
    crate::panes::views(pane)
        .next()
        .map(|view| (view.kind, view.target))
}

fn clock(cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx);
    let now = chrono::Local::now();
    div()
        .flex()
        .items_baseline()
        .font_features(crate::theme::tabular())
        .child(
            ui::text(
                now.format("%-I:%M").to_string(),
                TEXT_XS,
                FontWeight::SEMIBOLD,
            )
            .text_color(t.text_secondary),
        )
        .child(
            ui::text(now.format("%p").to_string(), TEXT_3XS, FontWeight::SEMIBOLD)
                .letter_spacing(0.06)
                .ml(SPACE_1)
                .text_color(t.text_tertiary),
        )
}

/// How long a tab asked to close stays closing in the window if the core never says it
/// went, such as when its closing failed.
const CLOSE_SETTLES: Duration = Duration::from_secs(2);

impl Workbench {
    /// Brings forward the project's tab showing `target`, opening one when none does.
    pub fn show(&self, kind: PaneKind, target: &str, title: Option<&str>) {
        if let Some(project) = self.project() {
            let tab = OpenTab::new(At::project(&project.id), kind, target);
            self.open_in_front(OpenTab {
                title: title.map(str::to_owned),
                ..tab
            });
        }
    }

    pub fn open_in_front(&self, tab: OpenTab) {
        self.target().call_typed::<call::OpenTab>(tab);
    }

    pub fn open_here(&self, tab: OpenTab) {
        Target::here(&self.asks).call_typed::<call::OpenTab>(tab);
    }

    /// Brings forward the tab `by` places from the one in front, among those `same`
    /// accepts, going round the strip.
    pub(crate) fn step_tab(&self, by: isize, same: impl Fn(&LayoutTab, &LayoutTab) -> bool) {
        let (Some(strip), Some(front)) = (self.strip(), self.tab()) else {
            return;
        };
        let tabs: Vec<&LayoutTab> = self
            .tabs(&strip)
            .iter()
            .filter(|tab| same(tab, front))
            .collect();
        let Some(at) = tabs.iter().position(|tab| tab.id == front.id) else {
            return;
        };
        let to = (at as isize + by).rem_euclid(tabs.len() as isize) as usize;
        self.activate_tab(&strip, &tabs[to].id);
    }

    /// Brings forward the tab at `place` in the strip, counting from one, or the last tab.
    pub(crate) fn go_to_tab(&self, place: Option<usize>) {
        let Some(strip) = self.strip() else {
            return;
        };
        let tabs = self.tabs(&strip);
        if let Some(tab) = place.map_or(tabs.last(), |place| tabs.get(place - 1)) {
            self.activate_tab(&strip, &tab.id);
        }
    }

    /// Brings forward the first terminal tab in the strip in front, else opens a terminal.
    pub(crate) fn focus_terminal(&self, cx: &mut App) {
        let terminal = self.strip().and_then(|strip| {
            let tab = self.tabs(&strip).iter().find(|tab| {
                tabs::first_view(&tab.pane).is_some_and(|(kind, _)| kind == PaneKind::Terminal)
            })?;
            Some((strip.clone(), tab.id.clone()))
        });
        match terminal {
            Some((strip, tab)) => self.activate_tab(&strip, &tab),
            None => self.new_terminal(cx),
        }
    }

    /// Makes the panes of the tab in front, for the kinds made the first time one shows, in
    /// whichever of its splits.
    pub(crate) fn make_front_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tell_reach(cx);
        let mut kinds: Vec<PaneKind> = Vec::new();
        if let Some(tab) = self.tab() {
            for view in crate::panes::views(&tab.pane) {
                if !kinds.contains(&view.kind) {
                    kinds.push(view.kind);
                }
            }
        }
        if kinds.contains(&PaneKind::Board) {
            if let (Some(strip), Some(tab)) = (self.strip(), self.tab().map(|tab| tab.id.clone())) {
                return self.close_tabs(&strip, &[tab]);
            }
        }
        for kind in kinds {
            crate::pane_kind::make(kind, self, window, cx);
        }
    }

    /// The strip in front: the terminals or SSH host shown instead of a project, else the
    /// project in front.
    pub fn strip(&self) -> Option<Strip> {
        let layout = &self.shown()?.layout;
        if layout.terminals_in_front {
            return Some(Strip::Terminals);
        }
        match layout
            .active_host
            .as_ref()
            .filter(|host| layout.hosts.contains_key(*host))
        {
            Some(host) => Some(Strip::Host(host.clone())),
            None => self
                .project()
                .map(|project| Strip::Project(project.id.clone())),
        }
    }

    pub(crate) fn layout_of(&self, strip: &Strip) -> Option<&ProjectLayout> {
        let layout = &self.shown()?.layout;
        match strip {
            Strip::Project(id) => layout.projects.get(id),
            Strip::Host(alias) => layout.hosts.get(alias),
            Strip::Terminals => Some(&layout.terminals),
        }
    }

    pub fn tabs(&self, strip: &Strip) -> &[LayoutTab] {
        self.layout_of(strip)
            .map_or(&[], |layout| layout.tabs.as_slice())
    }

    /// The tab in front in the strip in front.
    pub fn tab(&self) -> Option<&LayoutTab> {
        self.front_tab(&self.strip()?)
    }

    /// The tab in front in `strip`. While the one in front is closing, it is the one the core
    /// brings forward in its place: the latest open one the person had in front, else the next
    /// open tab after it, else the nearest before it.
    pub fn front_tab(&self, strip: &Strip) -> Option<&LayoutTab> {
        let layout = self.layout_of(strip)?;
        let tabs = layout.tabs.as_slice();
        let open = |tab: &&LayoutTab| !self.closing(&tab.id);
        let active = layout
            .active
            .as_deref()
            .and_then(|id| tabs.iter().position(|tab| tab.id == id));
        let recent = self
            .shown()
            .map_or(&[][..], |view| view.layout.recent.as_slice());
        let latest = || {
            recent
                .iter()
                .find_map(|id| tabs.iter().find(|tab| &tab.id == id).filter(open))
        };
        match active {
            Some(at) if open(&&tabs[at]) => Some(&tabs[at]),
            Some(at) => latest().or_else(|| {
                tabs[at..]
                    .iter()
                    .find(open)
                    .or_else(|| tabs[..at].iter().rev().find(open))
            }),
            None => tabs.iter().find(open),
        }
    }

    /// Whether `tab` was asked to close a moment ago and the layout has not caught up yet.
    pub(crate) fn closing(&self, tab: &str) -> bool {
        self.closing_tabs
            .borrow()
            .get(tab)
            .is_some_and(|asked| asked.elapsed() < CLOSE_SETTLES)
    }

    /// Forgets the tabs asked to close that the layout no longer lists, or that it has had
    /// time to answer for.
    pub(crate) fn settle_closing(&self) {
        let Some(layout) = self.shown().map(|view| &view.layout) else {
            return;
        };
        let listed = |id: &str| {
            std::iter::once(&layout.terminals)
                .chain(layout.projects.values())
                .chain(layout.hosts.values())
                .any(|strip| strip.tabs.iter().any(|tab| tab.id == id))
        };
        self.closing_tabs
            .borrow_mut()
            .retain(|id, asked| asked.elapsed() < CLOSE_SETTLES && listed(id));
    }

    pub fn activate_tab(&self, strip: &Strip, tab: &str) {
        if !self.closing(tab) {
            self.target()
                .call_typed::<call::ActivateTab>(TabParams::new(strip.at(), tab));
        }
    }

    /// Moves a tab to `to` in its strip, counted as the strip is now.
    pub fn move_tab(&self, strip: &Strip, tab: &str, to: usize) {
        if !self.closing(tab) {
            self.target().call_typed::<call::MoveTab>(TabParams {
                to: Some(to),
                ..TabParams::new(strip.at(), tab)
            });
        }
    }

    /// Keeps a tab opened to look at, so the next one opens beside it.
    pub fn keep_tab(&self, strip: &Strip, tab: &str) {
        if !self.closing(tab) {
            self.target()
                .call_typed::<call::KeepTab>(TabParams::new(strip.at(), tab));
        }
    }

    pub fn close_tabs(&self, strip: &Strip, tabs: &[String]) {
        let tabs: Vec<String> = (tabs.iter())
            .filter(|tab| !self.closing(tab))
            .cloned()
            .collect();
        if tabs.is_empty() {
            return;
        }
        let asked = std::time::Instant::now();
        self.closing_tabs
            .borrow_mut()
            .extend(tabs.iter().map(|tab| (tab.clone(), asked)));
        self.target().call_typed::<call::CloseTabs>(TabsParams {
            at: strip.at(),
            tabs,
        });
    }

    /// Names a tab; an empty name gives back the one its view gives it.
    pub fn rename_tab(&self, strip: &Strip, tab: &str, title: &str) {
        if !self.closing(tab) {
            self.target().call_typed::<call::RenameTab>(TabParams {
                title: Some(title.to_owned()),
                ..TabParams::new(strip.at(), tab)
            });
        }
    }

    /// Opens a copy of a tab beside it, with new shells for its terminals.
    pub fn duplicate_tab(&self, strip: &Strip, tab: &str) {
        if !self.closing(tab) {
            self.target()
                .call_typed::<call::DuplicateTab>(TabParams::new(strip.at(), tab));
        }
    }
}

/// Whether two tabs show the same kind of pane, as ⌃LayoutTab steps between them.
pub(crate) fn same_kind(tab: &LayoutTab, front: &LayoutTab) -> bool {
    let kind = |tab: &LayoutTab| tabs::first_view(&tab.pane).map(|(kind, _)| kind);
    kind(tab) == kind(front)
}

/// What the tab's first view of `kind` shows, among its splits.
pub fn shows(tab: &LayoutTab, kind: PaneKind) -> Option<&String> {
    crate::panes::views(&tab.pane)
        .find(|view| view.kind == kind)
        .map(|view| view.target)
}
