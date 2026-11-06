use crate::activity_view::calendar;
use crate::activity_view::Metric;

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, Context, Div, Entity, FontWeight, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, WeakEntity, Window,
};

use crate::agents::menu::AgentMenu;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_6, TEXT_2XS, TEXT_XL,
    TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, Button, IconButton, PaneHeader, Segmented, Variant};

use crate::agents::rows::AgentRow;
use crate::workbench::Workbench;
use crate::{chat, terminal};

/// The calendar's metrics, in the order its buttons show.
pub const METRICS: [(Metric, &str); 5] = [
    (Metric::Tokens, "Tokens"),
    (Metric::Cost, "Cost"),
    (Metric::Commits, "Commits"),
    (Metric::AgentTime, "Agent time"),
    (Metric::Sessions, "Sessions"),
];

/// The stage: the tab in front, its views side by side, or what a project with nothing
/// open says.
pub fn stage(bench: &Workbench, window: &Window, cx: &mut Context<Workbench>) -> AnyElement {
    if bench.settings.read(cx).is_open() {
        return crate::settings::view(bench, window, cx);
    }
    if let Some(goals) = bench.goals_in_front() {
        return goals.clone().into_any_element();
    }
    if bench
        .strip()
        .is_some_and(|strip| bench.tabs(&strip).is_empty())
    {
        return div()
            .flex_1()
            .min_w_0()
            .child(ui::pane(cx).child(empty_stage(cx)))
            .into_any_element();
    }
    let Some(tab) = bench.tab() else {
        return div().flex_1().min_w_0().into_any_element();
    };
    let shown = crate::track::stage(bench, tab, window, cx);
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .child(shown)
        .into_any_element()
}

/// One agent's view of a tab: its terminal or its chat under its header, or what a new
/// chat opens on.
pub fn agent_view(
    bench: &Workbench,
    tab: &str,
    view: &str,
    agent: &str,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let project = bench.project();
    let agent = project.and_then(|project| {
        bench
            .agents_of(&project.id)
            .into_iter()
            .find(|row| row.id == agent)
    });
    let header = agent
        .as_ref()
        .filter(|_| project.is_some())
        .map(|agent| pane_header(bench, agent, tab, view, cx));
    let shown = agent_shown(bench, agent.as_ref(), cx);
    under_header(header, shown, cx)
}

/// The agent in front as the goals show it: what its tab shows, under its header with a link
/// back in place of the view switch and the desk, which belong to the workbench.
pub fn agent_view_in_goals(
    bench: &Workbench,
    agent: &str,
    back: AnyElement,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    let agent = bench.agent().filter(|front| front.id == agent);
    let header = agent
        .as_ref()
        .map(|agent| agent_header(bench, agent, cx).back(back));
    let shown = agent_shown(bench, agent.as_ref(), cx);
    under_header(header, shown, cx)
}

/// What an agent's view shows under its header.
fn agent_shown(
    bench: &Workbench,
    agent: Option<&AgentRow>,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    if let Some((place, session)) = agent.and_then(|agent| bench.agent_terminal(&agent.id)) {
        let terminal = terminal::pane(bench, &cx.entity(), &place, &session.to_string(), cx);
        return div().size_full().child(terminal).into_any_element();
    }
    if let Some(agent) = agent.filter(|agent| bench.in_terminal(&agent.id)) {
        let asleep = bench
            .terminal_asleep(&agent.id)
            .then(|| asleep_in_terminal(bench, agent, cx));
        return div().size_full().children(asleep).into_any_element();
    }
    if let Some((agent, chat)) =
        agent.and_then(|agent| Some((agent, bench.chats.open.get(&agent.id)?)))
    {
        let resumes = bench.shown().is_some_and(|view| {
            view.workspace
                .agents
                .iter()
                .any(|saved| saved.id == agent.id && saved.resume_id.is_some())
        });
        let welcome = chat.is_empty(cx).then(|| {
            chat::status::connection_note(agent, chat, resumes, cx)
                .unwrap_or_else(|| welcome(bench, Some(agent), cx).into_any_element())
        });
        return chat::pane(bench, agent, chat, welcome, cx).into_any_element();
    }
    centred(welcome(bench, agent, cx))
        .size_full()
        .into_any_element()
}

/// An agent pane: its header across the top, and what it shows below it.
fn under_header(
    header: Option<impl IntoElement>,
    shown: impl IntoElement,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    ui::pane(cx)
        .flex()
        .flex_col()
        .children(header)
        .child(div().flex_1().min_h_0().p(px(ui::AGENT_INSET)).child(shown))
        .into_any_element()
}

/// A project with every tab closed, which would otherwise show nothing to click.
fn empty_stage(cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let hint = |id: &str| crate::keymap::hint(id, cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_3)
        .text_color(t.text_tertiary)
        .child(
            ui::text(
                "Nothing open in this project",
                TEXT_XS_H,
                FontWeight::NORMAL,
            )
            .text_color(t.text_secondary),
        )
        .child(
            div()
                .flex()
                .gap(SPACE_2)
                .child(
                    Button::new("empty-new-agent", "New agent", Variant::Outline)
                        .keys(hint("agent.new"))
                        .on_click(cx.listener(|bench, _, window, cx| bench.new_chat(window, cx))),
                )
                .child(
                    Button::new("empty-new-terminal", "New terminal", Variant::Outline)
                        .keys(hint("terminal.new"))
                        .on_click(cx.listener(|bench, _, _, cx| bench.new_terminal(cx))),
                )
                .child(
                    Button::new("empty-open-file", "Open file", Variant::Outline)
                        .keys(hint("palette.files"))
                        .on_click(cx.listener(|bench, _, window, cx| {
                            bench.toggle_finder(window, cx);
                            cx.notify();
                        })),
                ),
        )
}

/// A terminal agent put to sleep: its CLI starts again on its saved session when woken.
fn asleep_in_terminal(bench: &Workbench, agent: &AgentRow, cx: &mut Context<Workbench>) -> Div {
    let t = Tokens::get(cx).clone();
    let label = bench.agent_name(&agent.provider).to_owned();
    let id = agent.id.clone();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(SPACE_3)
        .child(
            ui::text(format!("{label} is asleep"), TEXT_XS_H, FontWeight::NORMAL)
                .text_color(t.text_secondary),
        )
        .child(
            Button::new("wake-terminal-agent", "Wake", Variant::Outline)
                .on_click(cx.listener(move |bench, _, _, _| bench.switch_view(&id, true))),
        )
}

/// What a chat opens on before its first message: the agent's face asking what is being made in
/// the project, over the calendar of the tokens burnt in it.
pub fn welcome(
    bench: &Workbench,
    agent: Option<&AgentRow>,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let mark = match agent {
        Some(agent) => agent.mark(32., cx),
        None => ui::mark("claude", 32.),
    };
    let project = bench.project().map(|project| project.name.clone());
    welcome_over(
        mark,
        project.unwrap_or_default(),
        bench.parts.activity.clone(),
        cx,
    )
    .max_w(px(sikemux_browser::REPLY_COLUMN_WIDTH))
    .mx(px(22.))
}

/// An agent's face, by `mark`, asking what is being made in `project`, over `under`, as wide as
/// it is given.
pub fn welcome_over(
    mark: AnyElement,
    project: impl Into<SharedString>,
    under: impl IntoElement,
    cx: &App,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(SPACE_6)
        .child(greeting(mark, project.into(), cx))
        .child(div().w_full().child(under))
}

/// An agent's face asking what is being made in the project.
fn greeting(mark: AnyElement, project: SharedString, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    div()
        .flex()
        .items_center()
        .justify_center()
        .gap(SPACE_3)
        .child(div().flex().child(mark))
        .child(
            div()
                .min_w_0()
                .flex()
                .items_center()
                .text_size(theme::ui_text(TEXT_XL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text_primary)
                .child("What are we making in")
                .child(
                    ui::icon(Icon::Folder, 17.)
                        .ml(SPACE_2)
                        .mr(SPACE_1H)
                        .text_color(t.live),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(theme::MONO)
                        .font_weight(FontWeight::NORMAL)
                        .text_size(theme::ui_text(18.))
                        .child(project),
                )
                .child("?"),
        )
}

/// The welcome in the middle of the space it is given, across and down.
pub fn centred(welcome: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .justify_center()
        .child(div().w_full().flex().justify_center().child(welcome))
}

/// The welcome's calendar of the tokens burnt each day: a view of its own, drawn again when the
/// workbench changes but not when a key is typed in the composer below it.
pub struct Activity {
    bench: WeakEntity<Workbench>,
    _bench: Subscription,
}

impl Activity {
    pub fn new(bench: &Entity<Workbench>, cx: &mut Context<Self>) -> Self {
        Self {
            bench: bench.downgrade(),
            _bench: cx.observe(bench, |_, _, cx| cx.notify()),
        }
    }
}

impl Render for Activity {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.bench
            .update(cx, |bench, cx| welcome_calendar(bench, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

fn welcome_calendar(bench: &Workbench, cx: &mut Context<Workbench>) -> AnyElement {
    let Some(summary) = bench.feeds.in_front().activity.as_ref() else {
        return div().into_any_element();
    };
    let t = Tokens::get(cx).clone();
    let more = div()
        .id("welcome-more")
        .group("welcome-more")
        .py(SPACE_0)
        .flex()
        .items_center()
        .gap(SPACE_1)
        .cursor_pointer()
        .text_size(theme::ui_text(TEXT_2XS))
        .text_color(t.ink_faint)
        .hover(|more| more.text_color(t.text_primary))
        .child("View activity")
        .child(
            ui::icon(Icon::Chevron, 10.)
                .text_color(t.ink_faint)
                .group_hover("welcome-more", move |icon| icon.text_color(t.text_primary)),
        )
        .on_click(cx.listener(|bench, _, window, cx| {
            bench.open_settings(crate::settings::Page::Usage, window, cx)
        }));
    calendar(
        summary,
        bench,
        &cx.entity(),
        Some(more.into_any_element()),
        cx,
    )
    .into_any_element()
}

fn pane_header(
    bench: &Workbench,
    agent: &AgentRow,
    tab: &str,
    view: &str,
    cx: &mut Context<Workbench>,
) -> impl IntoElement {
    let desk_open = bench
        .desk_of(tab, &agent.id)
        .is_some_and(|desk| !bench.panes.desk_shutting(&desk));
    let (tab, view, agent_id) = (tab.to_owned(), view.to_owned(), agent.id.clone());
    let views = vec![
        (Icon::Agent, SharedString::from("GUI")),
        (Icon::Terminal, SharedString::from("TUI")),
    ];
    let in_terminal = bench.in_terminal(&agent.id);
    let working = agent.busy;
    let has_chat = bench.has_chat(&agent.provider);
    let switched = agent.id.clone();
    agent_header(bench, agent, cx)
        .controls(
            Segmented::new("agent-view", views, usize::from(in_terminal))
                .slide()
                .disabled_because(match (has_chat, working) {
                    (false, _) => Some("This agent has no built-in chat".to_owned()),
                    (true, true) => {
                        let other = if in_terminal { "GUI" } else { "TUI" };
                        Some(format!(
                            "Wait for the agent to finish before opening {other}"
                        ))
                    }
                    (true, false) => None,
                })
                .on_select(cx.listener(move |bench, picked: &usize, _, _| {
                    let terminal = *picked == 1;
                    if terminal != in_terminal {
                        bench.switch_view(&switched, terminal);
                    }
                })),
        )
        .side(
            IconButton::new(
                "agent-desk",
                Icon::PanelRight,
                if desk_open {
                    "Close the desk"
                } else {
                    "Open the desk"
                },
            )
            .compact()
            .pressed(desk_open)
            .on_click(
                cx.listener(move |bench, _, _, cx| bench.toggle_desk(&tab, &view, &agent_id, cx)),
            ),
        )
}

/// What says which agent a view shows, wherever it shows: its mark, title and name, and what
/// can be done to it from anywhere.
fn agent_header(bench: &Workbench, agent: &AgentRow, cx: &mut Context<Workbench>) -> PaneHeader {
    let t = Tokens::get(cx).clone();
    let menu = AgentMenu::new(bench, &agent.id, cx);
    let more = KitButton::new("agent-more")
        .ghost()
        .compact()
        .w(px(24.))
        .h(px(24.))
        .tooltip("More")
        .child(ui::icon(Icon::More, 14.).text_color(t.text_secondary))
        .dropdown_menu_with_anchor(
            Anchor::TopRight,
            ui::menu(move |popup, _, _| match &menu {
                Some(menu) => menu.build(popup),
                None => popup,
            }),
        );
    let in_terminal = bench.in_terminal(&agent.id);
    let has_chat = bench.has_chat(&agent.provider);
    let shared = agent.id.clone();
    let open_share = cx.listener(move |bench, _, window, cx| bench.open_share(&shared, window, cx));
    let share = match bench.share_of(&agent.id) {
        Some(share) => {
            let viewers = share.viewers;
            div()
                .id("agent-share")
                .h(px(24.))
                .min_w(px(24.))
                .px(SPACE_1H)
                .flex()
                .items_center()
                .justify_center()
                .gap(px(3.))
                .rounded(crate::theme::RADIUS_2)
                .text_size(crate::theme::ui_text(TEXT_XS))
                .text_color(t.live)
                .tooltip(ui::tooltip(crate::share::watching(share)))
                .on_click(open_share)
                .child(ui::icon(Icon::Eye, 13.).text_color(t.live))
                .when(viewers > 0, |button| button.child(viewers.to_string()))
                .into_any_element()
        }
        None => IconButton::new("agent-share", Icon::Share, "Share this chat read-only")
            .compact()
            .on_click(open_share)
            .into_any_element(),
    };
    let id = agent.id.clone();
    let editing = bench
        .overlays
        .retitling
        .as_ref()
        .filter(|retitling| retitling.agent == agent.id && !retitling.in_rail)
        .map(|retitling| retitle_field(retitling, 13., cx).min_w(px(160.)));
    PaneHeader::new(agent.title.clone())
        .title_view(editing)
        .on_title_double_click(
            cx.listener(move |bench, _, window, cx| bench.retitle(&id, false, window, cx)),
        )
        .mark(match agent.face.is_some() {
            true => {
                let picked = agent.id.clone();
                div()
                    .id("agent-buddy")
                    .cursor_pointer()
                    .tooltip(ui::tooltip("Change buddy"))
                    .child(agent.mark(16., cx))
                    .on_click(
                        cx.listener(move |bench, _, _, cx| bench.open_buddy_picker(&picked, cx)),
                    )
                    .into_any_element()
            }
            false => ui::mark(&agent.provider, 16.).into_any_element(),
        })
        .name(
            agent
                .name
                .as_ref()
                .map(|name| SharedString::from(format!("@{name}"))),
        )
        .when_some(
            in_terminal
                .then(|| crate::worktrees::head::badges(bench, &agent.id, cx))
                .flatten(),
            |header, badges| header.action(badges),
        )
        .when(has_chat, |header| header.action(share))
        .action(more)
}

/// The field an agent's title is edited in, at `size`; Escape leaves the title as it was.
pub fn retitle_field(
    retitling: &crate::agents::retitle::Retitling,
    size: f32,
    cx: &mut Context<Workbench>,
) -> gpui_kit::Div {
    let t = Tokens::get(cx).clone();
    div()
        .key_context("Retitle")
        .on_action(
            cx.listener(|bench, _: &crate::keymap::CancelRetitle, _, cx| {
                bench.finish_retitle(false, cx)
            }),
        )
        .child(
            Input::new(&retitling.input)
                .appearance(false)
                .p_0()
                .text_size(crate::theme::ui_text(size))
                .text_color(t.text_primary),
        )
}
