//! What an agent's header says of its worktree's branch, as the Mac's does: the pull request
//! the branch has.

use crate::ui::Clicks;
use gpui_kit::{
    div, px, AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled,
};
use sikemux_calls::call;
use sikemux_calls::codehost::PullsParams;
use sikemux_plugin_api::codehost::{Pull, PullState};

use crate::core::Target;
use crate::icons::Icon;
use crate::reads::Read;
use crate::theme::{self, Tokens, SPACE_1H, TEXT_2XS};
use crate::ui;
use crate::workbench::Workbench;

impl Workbench {
    /// Points what the header of the agent in front reads at that agent's worktree, while it
    /// works in one.
    pub(crate) fn watch_worktree(&mut self) {
        let watched = self.agent().and_then(|agent| {
            let known = self
                .shown()?
                .workspace
                .agents
                .iter()
                .find(|known| known.id == agent.id)?;
            let folder = known.worktree.as_ref()?.path.to_str()?.to_owned();
            Some((agent.id, folder, self.place()))
        });
        if watched == self.watched.worktree {
            return;
        }
        if let Some((_, _, was)) = &self.watched.worktree {
            let left = Target::new(&self.asks, was.clone());
            left.unwatch(Read::WorktreePulls);
            self.feeds.at_mut(was).worktree_pulls = None;
        }
        let target = self.target();
        if let Some((_, folder, _)) = &watched {
            target.watch_folder::<call::CodeHostPulls>(
                Read::WorktreePulls,
                PullsParams {
                    project: folder.clone(),
                    state: Some("all".to_owned()),
                },
                folder.clone(),
            );
        }
        self.watched.worktree = watched;
    }
}

/// The branch's pull request: the open one when there is one, else the newest it had.
fn pull<'a>(bench: &'a Workbench, branch: &str) -> Option<&'a Pull> {
    let own: Vec<&Pull> = (bench.feeds.in_front().worktree_pulls.as_ref()?.pulls.iter())
        .filter(|listed| listed.own_branch && listed.pull.head.as_deref() == Some(branch))
        .map(|listed| &listed.pull)
        .collect();
    own.iter()
        .find(|pull| matches!(pull.listed_state(), PullState::Open | PullState::Draft))
        .or(own.first())
        .copied()
}

/// The pull request's badge for the agent in front, while its worktree's branch has one.
pub fn pull_badge(
    bench: &Workbench,
    agent: &str,
    cx: &mut Context<Workbench>,
) -> Option<AnyElement> {
    let (watched, _, _) = bench.watched.worktree.as_ref()?;
    if watched != agent {
        return None;
    }
    let known = bench
        .shown()?
        .workspace
        .agents
        .iter()
        .find(|known| known.id == agent)?;
    let worktree = known.worktree.as_ref()?;
    let t = Tokens::get(cx).clone();
    let pull = pull(bench, &worktree.branch)?;
    let chip = |id: &'static str| {
        div()
            .id(id)
            .h(px(20.))
            .px(SPACE_1H)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .rounded(theme::radius_2())
            .border(crate::theme::border())
            .cursor_pointer()
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .hover(|style| style.bg(t.surface_hover).border_color(t.border_strong))
    };
    let (number, title, url) = (pull.number, &pull.title, pull.url.clone());
    let state = pull.listed_state();
    let (_, color, word) = crate::code_host_marks::pull_look(state, &t);
    let edge = match state {
        PullState::Open | PullState::Merged => color.opacity(0.34),
        PullState::Closed | PullState::Draft => t.border,
    };
    let opened = agent.to_owned();
    Some(
        chip("agent-pull-badge")
            .border_color(edge)
            .text_color(color)
            .tooltip(ui::tooltip(format!("#{number} {title}")))
            .child(ui::icon(Icon::PullRequest, 12.).text_color(color))
            .child(ui::mono(format!("#{number}"), TEXT_2XS, cx).text_color(color))
            .child(word)
            .clicks(cx.listener(move |bench, _, _, cx| {
                bench.open_on_desk(&opened, &url, cx);
            }))
            .into_any_element(),
    )
}
