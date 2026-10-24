//! What an agent's header says of its worktree's branch, as the Mac's does: the pull request
//! the branch has, and a way to remove the worktree once its work is merged.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, px, AnyElement, Context, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled,
};
use serde_json::{json, Value};

use crate::core::Watch;
use crate::icons::Icon;
use crate::theme::{self, Tokens};
use crate::ui;
use crate::workbench::Workbench;

const PULLS: &str = "worktree-pulls";
const MERGED: &str = "worktree-merged";

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
            let folder = known.worktree.as_ref()?["path"].as_str()?.to_owned();
            Some((agent.id, folder, self.place()))
        });
        if watched == self.watched_worktree {
            return;
        }
        let target = self.target();
        for key in [PULLS, MERGED] {
            self.forget_read(key);
        }
        match &watched {
            Some((agent, folder, _)) => {
                target.watch(
                    PULLS,
                    Watch::new(
                        "codeHostPulls",
                        json!({ "project": folder, "state": "all" }),
                        &["codeHost"],
                    ),
                );
                target.watch(
                    MERGED,
                    Watch::new("worktreeMerged", json!({ "agentId": agent }), &["git"]),
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
fn pull<'a>(bench: &'a Workbench, branch: &str) -> Option<&'a Value> {
    let own: Vec<&Value> = bench.read(PULLS)?["pulls"]
        .as_array()?
        .iter()
        .filter(|listed| listed["ownBranch"] == true && listed["pull"]["head"] == branch)
        .map(|listed| &listed["pull"])
        .collect();
    own.iter()
        .find(|pull| pull["state"] == "open")
        .or(own.first())
        .copied()
}

fn merged(pull: &Value) -> bool {
    pull["state"] == "merged" || !pull["mergedAt"].is_null()
}

/// How a pull request reads: a draft still open reads as a draft, as GitHub shows it.
fn state(pull: &Value) -> &'static str {
    if merged(pull) {
        return "Merged";
    }
    match (pull["state"].as_str(), pull["draft"] == true) {
        (Some("open"), true) => "Draft",
        (Some("open"), false) => "Open",
        _ => "Closed",
    }
}

/// The pull request's badge and the button that removes a merged worktree, for the agent in
/// front, while its worktree has them.
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
    let branch = worktree["branch"].as_str()?.to_owned();
    let path = worktree["path"].as_str().unwrap_or_default().to_owned();
    let t = Tokens::get(cx).clone();
    let pull = pull(bench, &branch);
    let pull_merged = pull.is_some_and(merged);
    let can_remove = pull_merged || bench.read(MERGED).and_then(Value::as_bool) == Some(true);
    let chip = |id: &'static str| {
        div()
            .id(id)
            .h(px(24.))
            .px(px(8.))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(5.))
            .rounded(theme::RADIUS_2)
            .border_1()
            .cursor_pointer()
            .text_size(crate::theme::ui_text(12.))
            .hover(|style| style.bg(t.surface_hover).border_color(t.border_strong))
    };
    let badge = pull.and_then(|pull| {
        let number = pull["number"].as_u64()?;
        let title = pull["title"].as_str().unwrap_or_default();
        let url = pull["url"].as_str()?.to_owned();
        let state = state(pull);
        let (color, edge) = match state {
            "Open" => (t.live, t.live.opacity(0.34)),
            "Merged" => (t.accent, t.accent.opacity(0.34)),
            "Closed" => (t.danger, t.border),
            _ => (t.text_tertiary, t.border),
        };
        let opened = agent.to_owned();
        Some(
            chip("agent-pull-badge")
                .border_color(edge)
                .text_color(color)
                .tooltip(ui::tooltip(format!("#{number} {title}")))
                .child(ui::icon(Icon::PullRequest, 12.).text_color(color))
                .child(ui::mono(format!("#{number}"), 12., cx))
                .child(state)
                .on_click(cx.listener(move |bench, _, _, _| {
                    bench.open_on_desk(&opened, &url);
                })),
        )
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
            .gap(px(6.))
            .children(badge)
            .when_some(remove, |row, remove| row.child(remove))
            .into_any_element(),
    )
}
