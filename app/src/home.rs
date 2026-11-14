use crate::activity_view::Metric;
use crate::ui::Clicks;
use crate::workbench::actions;
use crate::workbench::actions::CommandId;

use gpui_kit::component::button::{Button as KitButton, ButtonVariants};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::DropdownMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, Anchor, AnyElement, App, Context, Div, FontWeight, InteractiveElement, IntoElement,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, Window,
};

use crate::agents::menu::AgentMenu;
use crate::icons::Icon;
use crate::theme::{
    self, Tokens, SPACE_1H, SPACE_2, SPACE_3, SPACE_6, TEXT_SM, TEXT_XL, TEXT_XS, TEXT_XS_H,
};
use crate::ui::{self, Button, IconButton, PaneHeader, Segmented, Variant};

use crate::agents::rows::AgentRow;
use crate::workbench::Workbench;
use crate::{chat, terminal};

/// The calendar's metrics, in the order its buttons show.
pub const METRICS: [(Metric, &str); 5] = [
    (Metric::Tokens, n!("Tokens")),
    (Metric::Cost, n!("Cost")),
    (Metric::Commits, n!("Commits")),
    (Metric::AgentTime, n!("Agent time")),
    (Metric::Sessions, n!("Sessions")),
];

/// The stage: the tab in front, its views side by side, or what a project with nothing
/// open says.
pub fn stage(bench: &Workbench, window: &Window, cx: &mut Context<Workbench>) -> AnyElement {
    GLOWING.set(false);
    let stage = stage_shown(bench, window, cx);
    crate::ui::grain::glow_shown(GLOWING.get(), cx);
    stage
}

thread_local! {
    /// Whether a view drawn in this pass of the stage shows a new chat's glow.
    static GLOWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn stage_shown(bench: &Workbench, window: &Window, cx: &mut Context<Workbench>) -> AnyElement {
    if bench.settings.read(cx).is_open() {
        return crate::settings::view(bench, window, cx);
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
    let glow = welcoming(bench, agent.as_ref(), cx);
    let shown = agent_shown(bench, agent.as_ref(), cx);
    under_header(header, shown, glow, cx)
}

/// How strong a new chat's glow is in an agent's view: whole while nothing is said in it, and
/// falling to nothing as its welcome leaves after the person's first message.
fn welcoming(bench: &Workbench, agent: Option<&AgentRow>, cx: &App) -> f32 {
    let Some(agent) = agent else {
        return 1.;
    };
    if bench.in_terminal(&agent.id) {
        return 0.;
    }
    match bench.chats.open.get(&agent.id) {
        Some(chat) => chat.welcome_gone(cx).map_or(0., |gone| 1. - gone),
        None => 1.,
    }
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
        let welcome = chat.welcome_gone(cx).map(|gone| {
            chat::status::connection_note(agent, chat, resumes, cx)
                .unwrap_or_else(|| welcome(bench, 1. - gone, cx).into_any_element())
        });
        return chat::pane(bench, agent, chat, welcome, cx).into_any_element();
    }
    centred(welcome(bench, 1., cx))
        .size_full()
        .into_any_element()
}

/// An agent pane: its header across the top, and what it shows below it, over a new chat's
/// glow as strong as `glow`, from 0 to 1, crossing over to the pane field as it falls.
fn under_header(
    header: Option<impl IntoElement>,
    shown: impl IntoElement,
    glow: f32,
    cx: &mut Context<Workbench>,
) -> AnyElement {
    if glow > 0. {
        GLOWING.set(true);
    }
    ui::pane_frame(cx)
        .flex()
        .flex_col()
        .child(crate::ui::grain::pane_ground(cx)(glow))
        .children(header)
        .child(div().flex_1().min_h_0().p(px(ui::AGENT_INSET)).child(shown))
        .into_any_element()
}

/// A project with every tab closed, which would otherwise show nothing to click.
fn empty_stage(cx: &mut Context<Workbench>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let hint = |id: CommandId| crate::keymap::hint(id, cx);
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
                t!("Nothing open in this project"),
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
                    Button::new("empty-new-agent", t!("New agent"), Variant::Outline)
                        .keys(hint(actions::NewChat::ID))
                        .on_click(cx.listener(|bench, _, window, cx| bench.new_chat(window, cx))),
                )
                .child(
                    Button::new("empty-new-terminal", t!("New terminal"), Variant::Outline)
                        .keys(hint(actions::NewTerminal::ID))
                        .on_click(|_, window, cx| {
                            crate::keymap::dispatch(actions::NewTerminal::ID, window, cx)
                        }),
                )
                .child(
                    Button::new("empty-open-file", t!("Open file"), Variant::Outline)
                        .keys(hint(actions::OpenFile::ID))
                        .on_click(|_, window, cx| {
                            crate::keymap::dispatch(actions::OpenFile::ID, window, cx)
                        }),
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
            ui::text(
                t!("{label} is asleep", label = label),
                TEXT_XS_H,
                FontWeight::NORMAL,
            )
            .text_color(t.text_secondary),
        )
        .child(
            Button::new("wake-terminal-agent", t!("Wake"), Variant::Outline)
                .on_click(cx.listener(move |bench, _, _, _| bench.switch_view(&id, true))),
        )
}

/// What a chat opens on before its first message: Sikemux's logo asking what is being made in
/// the project.
pub fn welcome(bench: &Workbench, shown: f32, cx: &mut Context<Workbench>) -> impl IntoElement {
    let project = bench.project().map(|project| project.name.clone());
    welcome_over(project.unwrap_or_default(), div(), shown, cx)
        .max_w(px(sikemux_browser::REPLY_COLUMN_WIDTH))
        .mx(px(22.))
}

/// Sikemux's mark in dots, asking what is being made in `project`, over `under`, as wide as it
/// is given and as solid as `shown`, from 0 to 1.
pub fn welcome_over(
    project: impl Into<SharedString>,
    under: impl IntoElement,
    shown: f32,
    cx: &App,
) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .gap(SPACE_6)
        .opacity(shown)
        .child(logo(shown, cx))
        .child(greeting(project.into(), cx))
        .child(div().w_full().child(under))
}

/// Sikemux's mark in dots of the accent, as the setup's poster draws it, its dots thinning
/// away as `shown` falls to 0.
fn logo(shown: f32, cx: &App) -> AnyElement {
    match crate::ui::grain::poster_mark(shown, LOGO_DOTS, cx) {
        Some(mark) => mark.size(px(LOGO_SIZE)).into_any_element(),
        None => ui::icon(Icon::Logo, LOGO_SIZE)
            .text_color(Tokens::get(cx).accent)
            .into_any_element(),
    }
}

const LOGO_SIZE: f32 = 110.;
/// How far apart the mark's dots are, in points.
const LOGO_DOTS: f32 = 2.;

/// Asking what is being made in the project.
fn greeting(project: SharedString, cx: &App) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let asked = crate::i18n::tr(n!("What are we making in {project}?"));
    let (before, after) = asked
        .split_once("{project}")
        .unwrap_or(("What are we making in", "?"));
    div().flex().items_center().justify_center().child(
        div()
            .min_w_0()
            .flex()
            .items_center()
            .text_size(theme::ui_text(TEXT_XL))
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.text_primary)
            .child(before.trim_end())
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
            .child(after),
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

/// The welcome across the middle and resting on the bottom of the space it is given, so it sits
/// just over a composer drawn below it.
pub fn over_composer(welcome: impl IntoElement) -> Div {
    centred(welcome).justify_end().pb(SPACE_6 + SPACE_3)
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
        .is_some_and(|desk| !bench.panes.shutting(&desk));
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
                    (false, _) => Some(t!("This agent has no built-in chat").to_owned()),
                    (true, true) => {
                        let other = if in_terminal { "GUI" } else { "TUI" };
                        Some(t!(
                            "Wait for the agent to finish before opening {other}",
                            other = other
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
                    t!("Close the desk")
                } else {
                    t!("Open the desk")
                },
            )
            .compact()
            .pressed(desk_open)
            .on_click(cx.listener(move |bench, _, _, cx| {
                crate::keymap::count(actions::ToggleDesk::ID.name(), cx);
                bench.toggle_desk(&tab, &view, &agent_id, cx)
            })),
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
        .tooltip(t!("More"))
        .child(ui::icon(Icon::More, 14.).text_color(t.text_secondary))
        .dropdown_menu_with_anchor(
            Anchor::TopRight,
            ui::menu(move |popup, window, cx| match &menu {
                Some(menu) => menu.build(popup, window, cx),
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
                .clicks(open_share)
                .child(ui::icon(Icon::Eye, 13.).text_color(t.live))
                .when(viewers > 0, |button| button.child(viewers.to_string()))
                .into_any_element()
        }
        None => IconButton::new("agent-share", Icon::Share, t!("Share this chat read-only"))
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
    let chief = agent.chief.then(|| chief_fact(bench, agent, cx));
    let chain = (!agent.chief).then(|| chain_to(bench, agent, cx)).flatten();
    let side = side_of(bench, agent, cx);
    PaneHeader::new(match agent.chief {
        true => "@Chief".to_owned(),
        false => agent.title.clone(),
    })
    .when_some(chain, PaneHeader::back)
    .when_some(chief, PaneHeader::fact)
    .title_view(editing)
    .on_title_double_click(
        cx.listener(move |bench, _, window, cx| bench.retitle(&id, false, window, cx)),
    )
    .mark(match agent.face.is_some() {
        true => {
            let picked = agent.id.clone();
            div()
                .id("agent-buddy")
                .tooltip(ui::tooltip(t!("Change buddy")))
                .child(agent.mark(16., cx))
                .clicks(cx.listener(move |bench, _, _, cx| bench.open_buddy_picker(&picked, cx)))
                .into_any_element()
        }
        false => ui::mark(&agent.provider, 16.).into_any_element(),
    })
    .name(
        agent
            .handle()
            .map(|name| SharedString::from(format!("@{name}"))),
    )
    .when_some(side, PaneHeader::fact)
    .when_some(
        agent.turn.as_ref().filter(|_| in_terminal),
        |header, turn| {
            header.fact(crate::turn_line::header(
                turn,
                &agent.provider,
                SharedString::from(format!("turn-head:{}", agent.id)),
            ))
        },
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

/// A side chat's tag naming the chat it was opened beside, which brings that chat to the front.
fn side_of(bench: &Workbench, agent: &AgentRow, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let main = bench.saved_agent(&agent.id)?.side_of.clone()?;
    let name = bench.agent_by_id(&main)?.name?;
    let t = Tokens::get(cx);
    Some(
        div()
            .id("side-of")
            .px(SPACE_1H)
            .rounded(crate::theme::RADIUS_1)
            .bg(t.surface_chosen)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .text_color(t.text_tertiary)
            .hover(|tag| tag.text_color(t.text_primary))
            .tooltip(ui::tooltip(t!("Go to the main chat")))
            .clicks(cx.listener(move |bench, _, _, cx| bench.open_agent(&main, cx)))
            .child(t!("side of @{name}", name = name))
            .into_any_element(),
    )
}

/// What follows @Chief in its chat's header: the project, how many agents it has, and the
/// agent and model it runs on, so its mark never reads as a model of Sikemux's own.
fn chief_fact(bench: &Workbench, agent: &AgentRow, cx: &App) -> AnyElement {
    let t = Tokens::get(cx);
    let project = bench.project();
    let count = project.map_or(0, |project| project.agents);
    let model = (bench.chats.open.get(&agent.id))
        .and_then(|chat| chat.state.as_ref()?.model.clone())
        .filter(|model| !model.current.is_empty())
        .map(|model| crate::chat::composer::chosen(&model, t!("Model")))
        .unwrap_or_else(|| crate::turn_line::maker(&agent.provider).to_owned());
    let dot = || div().text_color(t.text_disabled).child("·");
    div()
        .flex()
        .items_center()
        .gap(SPACE_1H)
        .text_size(crate::theme::ui_text(TEXT_XS))
        .text_color(t.text_tertiary)
        .children(project.map(|project| div().child(project.name.clone())))
        .child(dot())
        .child(match count {
            1 => t!("1 agent").to_owned(),
            count => t!("{count} agents", count = count),
        })
        .child(dot())
        .child(ui::mark(&agent.provider, 14.))
        .child(model)
        .into_any_element()
}

/// The chain down to a worker in a project with a chief, leading its header: @Chief, then each
/// lead above it, each with its own mark.
fn chain_to(bench: &Workbench, agent: &AgentRow, cx: &App) -> Option<AnyElement> {
    let project = bench.project().filter(|project| project.chief.is_some())?;
    let t = Tokens::get(cx);
    let agents = bench.agents_of(&project.id);
    let at = agents.iter().position(|row| row.id == agent.id)?;
    let lead = match agent.team {
        Some(sikemux_view::inbox::TeamLink::Member | sikemux_view::inbox::TeamLink::Last) => agents
            [..at]
            .iter()
            .rev()
            .find(|row| row.team == Some(sikemux_view::inbox::TeamLink::Head)),
        _ => None,
    };
    let step = |mark: AnyElement, name: String| {
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_1H)
            .child(ui::mark_box(16., mark))
            .child(name)
    };
    let chevron = || {
        ui::icon(Icon::Chevron, 10.)
            .flex_none()
            .text_color(t.text_disabled)
    };
    Some(
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(SPACE_1H)
            .text_size(crate::theme::ui_text(TEXT_SM))
            .text_color(t.text_secondary)
            .child(step(ui::chief_mark(16.), "@Chief".to_owned()))
            .child(chevron())
            .children(lead.map(|lead| {
                let name = lead
                    .name
                    .clone()
                    .map_or_else(|| lead.title.clone(), |name| format!("@{name}"));
                div()
                    .flex()
                    .items_center()
                    .gap(SPACE_1H)
                    .child(step(lead.mark(16., cx), name))
                    .child(chevron())
            }))
            .into_any_element(),
    )
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
