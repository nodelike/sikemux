//! An agent's worktree: what its header says of it, moving between worktrees, and removing
//! one.

pub(crate) mod head;
pub mod switch;

use gpui_kit::component::WindowExt;
use gpui_kit::{Context, Window};

use crate::toast;
use crate::workbench::Workbench;
use sikemux_core::server::call;
use sikemux_core::server::worktree::removal::{
    RemoveWorktreeParams, WorktreeParams, WorktreeRemovalInfo,
};

impl Workbench {
    /// Removes an agent's worktree once the person knows what that loses: uncommitted
    /// changes, and commits only its branch has, which a merged pull request already keeps.
    pub fn remove_worktree(
        &mut self,
        agent: &str,
        pull_merged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let agent = agent.to_owned();
        let cost = self
            .chat_target(&agent)
            .request_typed::<call::WorktreeRemoval>(WorktreeParams {
                agent_id: agent.clone(),
            });
        cx.spawn_in(window, async move |bench, cx| {
            let cost = cost.await;
            let _ = bench.update_in(cx, |bench, window, cx| match cost {
                Ok(cost) => bench.confirm_removal(agent, &cost, pull_merged, window, cx),
                Err(error) => window.push_notification(
                    toast::error(t!("Could not remove the worktree: {error}", error = error)),
                    cx,
                ),
            });
        })
        .detach();
    }

    pub(crate) fn confirm_removal(
        &mut self,
        agent: String,
        cost: &WorktreeRemovalInfo,
        pull_merged: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !cost.shared_with.is_empty() {
            return window.push_notification(
                t!("Another agent or project still uses this worktree. Close it before removing the worktree."),
                cx,
            );
        }
        if cost.busy {
            return window
                .push_notification(t!("Stop the agent's turn before removing its worktree"), cx);
        }
        let branch = cost.branch.clone();
        let path = cost.path.to_string_lossy().into_owned();
        let dirty = cost.dirty_files;
        let unmerged = !pull_merged && cost.unmerged_commits > 0;
        let named = branch.clone();
        let then_branch = move |bench: &mut Workbench,
                                discard: bool,
                                window: &mut Window,
                                cx: &mut Context<Workbench>| {
            let (agent, named) = (agent.clone(), named.clone());
            if !unmerged {
                return bench.finish_removal(agent, named, discard, false, window, cx);
            }
            bench.confirm(
                t!("Delete branch {named} too?", named = named),
                t!("It has commits that are not in the branch it started from. Deleting it loses any that were not pushed."),
                t!("Delete branch"),
                true,
                move |bench, delete, window, cx| {
                    bench.finish_removal(agent.clone(), named.clone(), discard, delete, window, cx)
                },
                cx,
            );
        };
        if dirty == 0 {
            return then_branch(self, false, window, cx);
        }
        let lost = match dirty {
            1 => t!("1 uncommitted change in {path} will be lost.", path = path),
            dirty => t!(
                "{count} uncommitted changes in {path} will be lost.",
                count = dirty,
                path = path
            ),
        };
        self.confirm(
            t!("Remove worktree {branch}?", branch = branch),
            lost,
            t!("Remove"),
            true,
            move |bench, remove, window, cx| {
                if remove {
                    then_branch(bench, true, window, cx);
                }
            },
            cx,
        );
    }

    pub(crate) fn finish_removal(
        &self,
        agent: String,
        branch: String,
        discard: bool,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let removing = self
            .chat_target(&agent)
            .request_typed::<call::RemoveAgentWorktree>(RemoveWorktreeParams {
                agent_id: agent.clone(),
                discard_changes: discard,
                delete_branch,
            });
        cx.spawn_in(window, async move |_, cx| {
            let removed = removing.await;
            let _ = cx.update(|window, cx| {
                window.push_notification(
                    match removed {
                        Ok(answer) if answer.branch_deleted => toast::success(t!(
                            "Removed worktree and branch {branch}",
                            branch = branch
                        )),
                        Ok(_) => toast::success(t!("Removed worktree {branch}", branch = branch)),
                        Err(error) => toast::error(t!(
                            "Could not remove worktree {branch}: {error}",
                            branch = branch,
                            error = error
                        )),
                    },
                    cx,
                )
            });
        })
        .detach();
    }
}
