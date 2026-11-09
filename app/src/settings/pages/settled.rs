//! The chats settled out of the rail's main list on the host in front, to find and restore.

use gpui_kit::component::input::InputState;
use gpui_kit::{
    div, AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, SharedString,
    Styled, Subscription, Window,
};
use sikemux_view::inbox::AgentShown;

use crate::icons::Icon;
use crate::settings::index::Page::Settled;
use crate::settings::index::{section, Entry};
use crate::settings::rows;
use crate::theme::{SPACE_2, SPACE_4};
use crate::ui;
use crate::workbench::Workbench;

/// What a search finds on this page.
pub(in crate::settings) const ENTRIES: &[Entry] = &[section(
    Settled,
    "Settled chats",
    "settled archived archive hidden done restore unsettle snooze",
)];

pub struct Fields {
    filter: Entity<InputState>,
    _typing: Subscription,
}

pub(super) fn make(bench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Find a settled chat"));
    let typing = super::follow_typing(bench, &filter, cx);
    let fields = Fields {
        filter,
        _typing: typing,
    };
    super::change_pages(bench, cx, |pages, _| pages.settled = Some(fields));
}

/// Who settled it and when, after its name.
fn about(agent: &AgentShown) -> SharedString {
    let when = agent.settled_at.map(ui::time::ago).unwrap_or_default();
    let by = match agent.auto_settled {
        true => " by Sikemux",
        false => "",
    };
    match &agent.name {
        Some(name) => format!("@{name} · {when}{by}"),
        None => format!("{when}{by}"),
    }
    .into()
}

fn restore(agent: &AgentShown, this: &Entity<Workbench>) -> AnyElement {
    let (restoring, id) = (this.clone(), agent.agent_id.clone());
    ui::Button::new(
        SharedString::from(format!("restore-{}", agent.agent_id)),
        "Restore",
        ui::Variant::Outline,
    )
    .on_click(move |_, _, cx| {
        restoring.update(cx, |bench, cx| {
            bench.settle_agents(vec![id.clone()], false, cx)
        })
    })
    .into_any_element()
}

pub fn render(
    bench: &Workbench,
    pages: &super::State,
    lit: &dyn Fn(&str) -> bool,
    this: &Entity<Workbench>,
    cx: &App,
) -> Vec<AnyElement> {
    let Some(fields) = &pages.settled else {
        return Vec::new();
    };
    let query = fields.filter.read(cx).value().trim().to_lowercase();
    let mut settled: Vec<&AgentShown> = bench
        .shown()
        .into_iter()
        .flat_map(|view| view.inbox.agents.iter().chain(&view.inbox.kept))
        .filter(|agent| agent.settled_at.is_some())
        .filter(|agent| {
            agent.title.to_lowercase().contains(&query)
                || agent
                    .name
                    .as_ref()
                    .is_some_and(|name| name.to_lowercase().contains(&query))
        })
        .collect();
    settled.sort_by_key(|agent| std::cmp::Reverse(agent.settled_at));
    let field = div()
        .px(SPACE_4)
        .pb(SPACE_2)
        .child(ui::Field::new(&fields.filter).icon(Icon::Search));
    let mut sections = vec![rows::section(
        "Settled chats",
        None,
        Some(
            "Chats you or Sikemux settled out of the rail. One comes back by itself when its agent asks you something, or when you write to it."
                .into(),
        ),
        lit("Settled chats"),
        div().child(field).children(settled.is_empty().then(|| {
            rows::note(
                match query.is_empty() {
                    true => "Nothing is settled.",
                    false => "No settled chat matches.",
                },
                cx,
            )
        })),
        cx,
    )];
    for project in bench.projects() {
        let in_project: Vec<&&AgentShown> = (settled.iter())
            .filter(|agent| agent.project.as_ref() == Some(&project.id))
            .collect();
        if in_project.is_empty() {
            continue;
        }
        let body = div().children(in_project.into_iter().map(|agent| {
            let mark = ui::agent_mark(
                &agent.agent_id,
                &agent.cli,
                agent.face.as_ref(),
                agent.group,
                16.,
                cx,
            );
            rows::row_with(
                Some(mark),
                agent.title.clone(),
                Some(about(agent)),
                restore(agent, this),
                cx,
            )
        }));
        sections.push(rows::section(
            project.name.clone(),
            None,
            None,
            false,
            body.into_any_element(),
            cx,
        ));
    }
    sections
}
