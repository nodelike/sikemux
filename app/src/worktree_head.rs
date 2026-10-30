//! What an agent's header says of its worktree's branch, as the Mac's does: the pull request
//! the branch has, and a way to remove the worktree once its work is merged.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled,
};
use serde_json::json;
use sikemux_plugin_api::codehost::Pull;
use sikemux_wire::changed::Topic;

use crate::core::Watch;
use crate::icons::Icon;
use crate::theme::{self, Tokens, SPACE_1H, SPACE_2, TEXT_XS};
use crate::ui;
use crate::workbench::Workbench;

pub(crate) const PULLS: &str = "worktree-pulls";
pub(crate) const MERGED: &str = "worktree-merged";

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
        if watched == self.watched_worktree {
            return;
        }
        let target = self.target();
        for key in [PULLS, MERGED] {
            self.reads.forget(key);
        }
        match &watched {
            Some((agent, folder, _)) => {
                target.watch(
                    PULLS,
                    Watch::new(
                        "codeHostPulls",
                        json!({ "project": folder, "state": "all" }),
                        &[Topic::CodeHost],
                    ),
                );
                let merged =
                    Watch::new("worktreeMerged", json!({ "agentId": agent }), &[Topic::Git]);
                let project =
                    (self.project()).map(|project| project.path.to_string_lossy().into_owned());
                target.watch(
                    MERGED,
                    match project {
                        Some(project) => merged.in_folder(project),
                        None => merged,
                    },
                );
            }
            None => {
                for key in [PULLS, MERGED] {
                    target.unwatch(key);
                }
            }
        }
        self.watched_worktree = watched;
    }
}

/// The branch's pull request: the open one when there is one, else the newest it had.
fn pull<'a>(bench: &'a Workbench, branch: &str) -> Option<&'a Pull> {
    let own: Vec<&Pull> = (bench.reads.worktree_pulls.as_ref()?.pulls.iter())
        .filter(|listed| listed.own_branch && listed.pull.head.as_deref() == Some(branch))
        .map(|listed| &listed.pull)
        .collect();
    own.iter()
        .find(|pull| pull.state == "open")
        .or(own.first())
        .copied()
}

fn merged(pull: &Pull) -> bool {
    pull.state == "merged" || pull.merged_at.is_some()
}

/// How a pull request reads: a draft still open reads as a draft, as GitHub shows it.
fn state(pull: &Pull) -> &'static str {
    if merged(pull) {
        return "Merged";
    }
    match (pull.state.as_str(), pull.draft) {
        ("open", true) => "Draft",
        ("open", false) => "Open",
        _ => "Closed",
    }
}

/// The pull request's badge and the button that removes a merged worktree, for the agent in
/// front, while its worktree has them. A teammate that lands as it goes reads as merged after
/// each land, so the button waits until the agent is done.
pub fn badges(bench: &Workbench, agent: &str, cx: &mut Context<Workbench>) -> Option<AnyElement> {
    let (watched, _, _) = bench.watched_worktree.as_ref()?;
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
    let branch = worktree.branch.clone();
    let path = worktree.path.to_string_lossy().into_owned();
    let t = Tokens::get(cx).clone();
    let pull = pull(bench, &branch);
    let pull_merged = pull.is_some_and(merged);
    let at_rest = bench
        .agent_by_id(agent)
        .is_none_or(|row| !row.busy && !row.background);
    let can_remove = at_rest && (pull_merged || bench.reads.worktree_merged == Some(true));
    let chip = |id: &'static str| {
        div()
            .id(id)
            .h(px(24.))
            .px(SPACE_2)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .rounded(theme::RADIUS_2)
            .border_1()
            .cursor_pointer()
            .text_size(crate::theme::ui_text(TEXT_XS))
            .hover(|style| style.bg(t.surface_hover).border_color(t.border_strong))
    };
    let badge = pull.map(|pull| {
        let (number, title, url) = (pull.number, &pull.title, pull.url.clone());
        let state = state(pull);
        let (color, edge) = match state {
            "Open" => (t.live, t.live.opacity(0.34)),
            "Merged" => (t.accent, t.accent.opacity(0.34)),
            "Closed" => (t.danger, t.border),
            _ => (t.text_tertiary, t.border),
        };
        let opened = agent.to_owned();
        chip("agent-pull-badge")
            .border_color(edge)
            .text_color(color)
            .tooltip(ui::tooltip(format!("#{number} {title}")))
            .child(ui::icon(Icon::PullRequest, 12.).text_color(color))
            .child(ui::mono(format!("#{number}"), TEXT_XS, cx))
            .child(state)
            .on_click(cx.listener(move |bench, _, _, cx| {
                bench.open_on_desk(&opened, &url, cx);
            }))
    });
    let removed = agent.to_owned();
    let remove = can_remove.then(|| {
        chip("agent-worktree-remove")
            .border_color(t.border)
            .text_color(t.text_tertiary)
            .tooltip(ui::tooltip(format!(
                "{branch} is merged. Remove its worktree at {path}."
            )))
            .child("Remove worktree")
            .on_click(cx.listener(move |bench, _, window, cx| {
                bench.remove_worktree(&removed, pull_merged, window, cx)
            }))
    });
    if badge.is_none() && remove.is_none() {
        return None;
    }
    Some(
        div()
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .children(badge)
            .when_some(remove, |row, remove| row.child(remove))
            .into_any_element(),
    )
}
