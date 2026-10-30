//! The Branches page: the local branches, then each remote with its branches while open,
//! what can be done with each, and the card that heads a branch's review.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{
    div, list, px, Anchor, AnyElement, App, Context, Div, Entity, FontWeight, HoverListenerMode,
    Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement,
    Styled,
};
use std::collections::HashMap;

use serde_json::{json, Value};
use sikemux_core::git::branches::GitBranch;
use sikemux_core::git::remote::{GitRemote, GitRemoteBranch};
use sikemux_plugin_api::codehost::{PullList, PullState};

use super::{row_button, GitPane, Pick, ROW_HEIGHT};
use crate::codehost::Area;
use crate::icons::Icon;
use crate::motion;
use crate::theme::{
    self, Tokens, RADIUS_2, SPACE_0, SPACE_1, SPACE_1H, SPACE_2, SPACE_3, SPACE_3H, SPACE_4,
    SPACE_5, TEXT_2XS, TEXT_3XS, TEXT_LG, TEXT_SM, TEXT_XS, TEXT_XS_H,
};
use crate::ui;

#[derive(Clone, PartialEq)]
pub(super) enum BranchEntry {
    /// The Local group's head, which folds its branches.
    Local {
        count: usize,
    },
    Branch(GitBranch),
    Remote {
        remote: GitRemote,
        open: bool,
    },
    RemoteBranch {
        remote: String,
        branch: GitRemoteBranch,
    },
}

impl BranchEntry {
    /// The branch or ref the entry reviews.
    pub(super) fn rev(&self) -> Option<&str> {
        match self {
            BranchEntry::Branch(branch) => Some(&branch.name),
            BranchEntry::RemoteBranch { branch, .. } => Some(&branch.full_ref),
            _ => None,
        }
    }
}

/// What the Branches page knows beyond the overview: the remotes, and the branches of
/// the one open.
#[derive(Default)]
pub(super) struct Branches {
    pub(super) remotes: Vec<GitRemote>,
    /// The remotes have been read once, so an empty list means the repository has none.
    pub(super) remotes_read: bool,
    pub(super) open: Option<String>,
    pub(super) remote_branches: Vec<GitRemoteBranch>,
    pub(super) local_folded: bool,
    pub(super) chosen: usize,
    pub(super) entries: Vec<BranchEntry>,
    /// The open pull request of each branch of the repository's own, by branch.
    pub(super) pulls: HashMap<String, Opened>,
}

/// A branch's open pull request: its number, title and whether it is a draft.
#[derive(Clone, PartialEq)]
pub(super) struct Opened {
    number: u64,
    title: String,
    draft: bool,
}

impl GitPane {
    fn local_branches(&self) -> &[GitBranch] {
        match &self.overview {
            Some(Ok(overview)) => &overview.branches,
            _ => &[],
        }
    }

    pub(super) fn current_branch(&self) -> String {
        self.local_branches()
            .iter()
            .find(|branch| branch.current)
            .map(|branch| branch.name.clone())
            .or_else(|| match &self.overview {
                Some(Ok(overview)) if !overview.status.branch.is_empty() => {
                    Some(overview.status.branch.clone())
                }
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Lists the branches again from what is known of them.
    pub(super) fn list_branches(&mut self) {
        let query = self.filters[super::Page::Branches as usize].query.clone();
        let found = |name: &str| query.is_empty() || name.to_lowercase().contains(&query);
        let mut entries = Vec::new();
        let local: Vec<GitBranch> = self
            .local_branches()
            .iter()
            .filter(|branch| found(&branch.name))
            .cloned()
            .collect();
        if !local.is_empty() {
            entries.push(BranchEntry::Local { count: local.len() });
            if !self.branches.local_folded {
                entries.extend(local.into_iter().map(BranchEntry::Branch));
            }
        }
        for remote in &self.branches.remotes {
            let open = self.branches.open.as_deref() == Some(remote.name.as_str());
            entries.push(BranchEntry::Remote {
                remote: remote.clone(),
                open,
            });
            if open {
                entries.extend(
                    self.branches
                        .remote_branches
                        .iter()
                        .filter(|branch| !branch.is_head_pointer && found(&branch.name))
                        .map(|branch| BranchEntry::RemoteBranch {
                            remote: remote.name.clone(),
                            branch: branch.clone(),
                        }),
                );
            }
        }
        super::super::relist(&self.branch_rows, &self.branches.entries, &entries);
        self.branches.entries = entries;
        self.branches.chosen = self
            .branches
            .chosen
            .min(self.branches.entries.len().saturating_sub(1));
        if matches!(
            self.branches.entries.get(self.branches.chosen),
            Some(BranchEntry::Local { .. })
        ) {
            self.branches.chosen += 1;
        }
    }

    /// Reads the remotes, and the open remote's branches, again.
    pub(super) fn read_remotes(&mut self, cx: &mut Context<Self>) {
        let answer = self.core.request("git.remotes", self.repo_call(json!({})));
        cx.spawn(async move |pane, cx| {
            let remotes: Vec<GitRemote> = answer
                .await
                .ok()
                .and_then(|remotes| serde_json::from_value(remotes).ok())
                .unwrap_or_default();
            pane.update(cx, |pane, cx| {
                pane.branches.remotes = remotes;
                pane.branches.remotes_read = true;
                pane.list_branches();
                cx.notify();
            })
            .ok();
        })
        .detach();
        if let Some(remote) = self.branches.open.clone() {
            self.read_remote_branches(remote, cx);
        }
        self.read_branch_pulls(cx);
    }

    /// Reads which branches have an open pull request on the code host, if there is one.
    pub(super) fn read_branch_pulls(&mut self, cx: &mut Context<Self>) {
        if self.host_of().is_none() {
            return;
        }
        let params = json!({ "project": self.repo, "state": "open" });
        let answer = self.core.request("codeHostPulls", params);
        cx.spawn(async move |pane, cx| {
            let list: Option<PullList> = answer
                .await
                .ok()
                .and_then(|answer| crate::codehost::answer(&answer).ok());
            let pulls = list
                .into_iter()
                .flat_map(|list| list.pulls)
                .filter(|listed| listed.own_branch)
                .filter_map(|listed| {
                    let opened = Opened {
                        number: listed.pull.number,
                        title: listed.pull.title,
                        draft: listed.state == PullState::Draft,
                    };
                    Some((listed.pull.head?, opened))
                })
                .collect();
            pane.update(cx, |pane, cx| {
                pane.branches.pulls = pulls;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn read_remote_branches(&mut self, remote: String, cx: &mut Context<Self>) {
        let params = self.repo_call(json!({ "remote": remote }));
        let answer = self.core.request("git.remoteBranches", params);
        cx.spawn(async move |pane, cx| {
            let branches: Vec<GitRemoteBranch> = answer
                .await
                .ok()
                .and_then(|branches| serde_json::from_value(branches).ok())
                .unwrap_or_default();
            pane.update(cx, |pane, cx| {
                if pane.branches.open.as_deref() == Some(remote.as_str()) {
                    pane.branches.remote_branches = branches;
                    pane.list_branches();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn toggle_remote(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.branches.open.as_deref() == Some(name) {
            self.branches.open = None;
        } else {
            self.branches.open = Some(name.to_owned());
            self.branches.remote_branches.clear();
            self.read_remote_branches(name.to_owned(), cx);
        }
        self.list_branches();
        cx.notify();
    }

    pub(super) fn choose_branch(&mut self, index: usize, cx: &mut Context<Self>) {
        self.branches.chosen = index;
        self.branch_rows.scroll_to_reveal_item(index);
        self.follow(cx);
        cx.notify();
    }

    pub(super) fn chosen_branch(&self) -> Option<BranchEntry> {
        self.branches.entries.get(self.branches.chosen).cloned()
    }

    fn step_branch(&mut self, step: isize, cx: &mut Context<Self>) {
        let count = self.branches.entries.len() as isize;
        if count == 0 {
            return;
        }
        let mut next = (self.branches.chosen as isize + step).clamp(0, count - 1) as usize;
        if matches!(
            self.branches.entries.get(next),
            Some(BranchEntry::Local { .. })
        ) {
            next = (next as isize + step.signum()).clamp(0, count - 1) as usize;
        }
        self.choose_branch(next, cx);
    }

    fn git(&mut self, label: String, method: &'static str, extra: Value, cx: &mut Context<Self>) {
        let params = self.repo_call(extra);
        self.run(label, method, params, cx, |pane, answer, cx| {
            if answer.is_ok() {
                pane.read_remotes(cx);
            }
        });
    }

    pub(super) fn checkout(&mut self, name: String, cx: &mut Context<Self>) {
        self.git(
            format!("checking out {name}"),
            "git.checkout",
            json!({ "branch": name }),
            cx,
        );
    }

    fn merge(&mut self, reference: String, squash: bool, cx: &mut Context<Self>) {
        let (label, method) = if squash {
            (format!("squash merging {reference}…"), "git.mergeSquash")
        } else {
            (format!("merging {reference}…"), "git.merge")
        };
        self.git(label, method, json!({ "branch": reference }), cx);
    }

    fn merge_menu(&mut self, reference: String, cx: &mut Context<Self>) {
        let into = self.current_branch();
        let (regular, squash) = (reference.clone(), reference.clone());
        self.menu(
            format!(
                "Merge {reference} into {}",
                if into.is_empty() { "HEAD" } else { &into }
            ),
            vec![
                Pick::new('m', "regular merge", move |pane, cx| {
                    pane.merge(regular.clone(), false, cx)
                }),
                Pick::new('s', "squash merge", move |pane, cx| {
                    pane.merge(squash.clone(), true, cx)
                }),
            ],
            cx,
        );
    }

    pub(super) fn new_branch(&mut self, start: Option<String>, cx: &mut Context<Self>) {
        let title = match &start {
            Some(start) => format!("New branch from {start}"),
            None => "New branch".to_owned(),
        };
        self.prompt(
            title,
            "branch name",
            "",
            move |pane, name, cx| {
                if !name.is_empty() {
                    pane.git(
                        "creating branch…".into(),
                        "git.branchCreate",
                        json!({ "name": name, "startPoint": start }),
                        cx,
                    );
                }
            },
            cx,
        );
    }

    pub(super) fn checkout_by_name(&mut self, cx: &mut Context<Self>) {
        self.prompt(
            "Checkout branch",
            "branch name (- for previous)",
            "",
            |pane, name, cx| {
                if !name.is_empty() {
                    pane.checkout(name, cx);
                }
            },
            cx,
        );
    }

    fn rename_branch(&mut self, branch: &GitBranch, cx: &mut Context<Self>) {
        let old = branch.name.clone();
        self.prompt(
            format!("Rename branch · {old}"),
            "",
            old.clone(),
            move |pane, name, cx| {
                if !name.is_empty() && name != old {
                    pane.git(
                        format!("renaming branch {old} → {name}"),
                        "git.branchRename",
                        json!({ "oldName": old, "newName": name }),
                        cx,
                    );
                }
            },
            cx,
        );
    }

    fn delete_menu(&mut self, branch: &GitBranch, cx: &mut Context<Self>) {
        let delete = |force: bool| {
            let name = branch.name.clone();
            move |pane: &mut GitPane, cx: &mut Context<GitPane>| {
                let name = name.clone();
                let (title, body, yes) = if force {
                    (
                        format!("Force delete {name}?"),
                        "This deletes the local branch even if it has commits that are not merged anywhere else.",
                        "force delete",
                    )
                } else {
                    (
                        format!("Delete {name}?"),
                        "Deletes the local branch only. Use force delete if Git refuses because the branch is not merged.",
                        "delete",
                    )
                };
                pane.ask(
                    title,
                    body,
                    yes,
                    move |pane, cx| {
                        pane.git(
                            format!(
                                "{}deleting branch {name}",
                                if force { "force " } else { "" }
                            ),
                            "git.branchDelete",
                            json!({ "name": name, "force": force }),
                            cx,
                        )
                    },
                    cx,
                );
            }
        };
        let current = branch.current;
        let mut keep = Pick::new('d', "delete local branch", delete(false)).destructive();
        let mut force = Pick::new('D', "force delete local branch", delete(true)).destructive();
        if current {
            keep = keep.hint("(can't delete current branch)");
            force = force.hint("(can't delete current branch)");
            keep.disabled = true;
            force.disabled = true;
        } else {
            force = force.hint("git branch -D");
        }
        self.menu(format!("Delete {}", branch.name), vec![keep, force], cx);
    }

    fn checkout_remote(&mut self, remote: String, branch: GitRemoteBranch, cx: &mut Context<Self>) {
        self.git(
            format!("checking out {}", branch.full_ref),
            "git.checkoutRemoteBranch",
            json!({ "remote": remote, "branch": branch.name, "localName": branch.tracked_by }),
            cx,
        );
    }

    fn set_upstream(&mut self, branch: &GitRemoteBranch, cx: &mut Context<Self>) {
        let current = self.current_branch();
        if current.is_empty() {
            return;
        }
        self.git(
            format!("setting upstream of {current}"),
            "git.setUpstream",
            json!({ "branch": current, "upstream": branch.full_ref }),
            cx,
        );
    }

    fn delete_remote_branch(
        &mut self,
        remote: String,
        branch: &GitRemoteBranch,
        cx: &mut Context<Self>,
    ) {
        let (full, name) = (branch.full_ref.clone(), branch.name.clone());
        self.ask(
            format!("Delete {full}?"),
            format!("Pushes a delete to {remote}. The branch will be gone for everyone."),
            "delete on remote",
            move |pane, cx| {
                pane.git(
                    format!("deleting {full}"),
                    "git.deleteRemoteBranch",
                    json!({ "remote": remote, "branch": name }),
                    cx,
                )
            },
            cx,
        );
    }

    fn edit_remote_url(&mut self, remote: &GitRemote, cx: &mut Context<Self>) {
        let (name, was) = (remote.name.clone(), remote.url.clone());
        self.prompt(
            format!("Edit url · {name}"),
            "",
            was.clone(),
            move |pane, url, cx| {
                if url.is_empty() || url == was {
                    return;
                }
                let params = pane.repo_call(json!({ "name": name, "url": url }));
                pane.run(
                    format!("setting url for {name}"),
                    "git.remoteSetUrl",
                    params,
                    cx,
                    |pane, _, cx| pane.read_remotes(cx),
                );
            },
            cx,
        );
    }

    fn rename_remote(&mut self, remote: &GitRemote, cx: &mut Context<Self>) {
        let was = remote.name.clone();
        self.prompt(
            format!("Rename remote · {was}"),
            "",
            was.clone(),
            move |pane, name, cx| {
                if name.is_empty() || name == was {
                    return;
                }
                let params = pane.repo_call(json!({ "oldName": was, "newName": name }));
                let (old, new) = (was.clone(), name.clone());
                pane.run(
                    format!("renaming {was} → {name}"),
                    "git.remoteRename",
                    params,
                    cx,
                    move |pane, _, cx| {
                        if pane.branches.open.as_deref() == Some(old.as_str()) {
                            pane.branches.open = Some(new.clone());
                        }
                        pane.read_remotes(cx)
                    },
                );
            },
            cx,
        );
    }

    fn remove_remote(&mut self, remote: &GitRemote, cx: &mut Context<Self>) {
        let name = remote.name.clone();
        self.ask(
            format!("Remove remote {name}?"),
            "Removes the local remote configuration. Won't touch the upstream repo.",
            "remove",
            move |pane, cx| {
                let params = pane.repo_call(json!({ "name": name }));
                pane.run(
                    format!("removing remote {name}"),
                    "git.remoteRemove",
                    params,
                    cx,
                    |pane, _, cx| pane.read_remotes(cx),
                );
            },
            cx,
        );
    }

    fn fetch_remote(&mut self, remote: Option<String>, cx: &mut Context<Self>) {
        let label = match &remote {
            Some(remote) => format!("fetching {remote}…"),
            None => "fetching all remotes…".to_owned(),
        };
        self.git(label, "git.fetch", json!({ "remote": remote }), cx);
    }

    /// Does what Enter does on the entry: checks a branch out, or opens a remote.
    fn activate_branch(&mut self, cx: &mut Context<Self>) {
        match self.chosen_branch() {
            Some(BranchEntry::Branch(branch)) if !branch.current => self.checkout(branch.name, cx),
            Some(BranchEntry::Remote { remote, .. }) => self.toggle_remote(&remote.name, cx),
            Some(BranchEntry::RemoteBranch { remote, branch }) => {
                self.checkout_remote(remote, branch, cx)
            }
            Some(BranchEntry::Local { .. }) => {
                self.branches.local_folded = !self.branches.local_folded;
                self.list_branches();
                cx.notify();
            }
            _ => {}
        }
    }

    /// The Branches page's keys; false for one it leaves to the pane.
    pub(super) fn branch_key(&mut self, typed: &str, cx: &mut Context<Self>) -> bool {
        let chosen = self.chosen_branch();
        match (typed, chosen) {
            ("j" | "down", _) => self.step_branch(1, cx),
            ("k" | "up", _) => self.step_branch(-1, cx),
            ("enter" | "space" | " ", _) => self.activate_branch(cx),
            ("n", entry) => {
                let start = entry.as_ref().and_then(BranchEntry::rev).map(str::to_owned);
                self.new_branch(start, cx)
            }
            ("N", _) => self.new_branch(None, cx),
            ("c", _) => self.checkout_by_name(cx),
            ("M", Some(entry)) if entry.rev().is_some() => {
                let reference = entry.rev().unwrap_or_default().to_owned();
                self.merge_menu(reference, cx)
            }
            ("d", Some(BranchEntry::Branch(branch))) => self.delete_menu(&branch, cx),
            ("d", Some(BranchEntry::RemoteBranch { remote, branch })) => {
                self.delete_remote_branch(remote, &branch, cx)
            }
            ("R", Some(BranchEntry::Branch(branch))) => self.rename_branch(&branch, cx),
            ("u", Some(BranchEntry::RemoteBranch { branch, .. })) => self.set_upstream(&branch, cx),
            ("f", Some(BranchEntry::Remote { remote, .. })) => {
                self.fetch_remote(Some(remote.name), cx)
            }
            ("f", Some(BranchEntry::RemoteBranch { remote, .. })) => {
                self.fetch_remote(Some(remote), cx)
            }
            ("f", _) => self.fetch_remote(None, cx),
            _ => return false,
        }
        true
    }
}

/// A line of a "More" menu: what it says, its icon and key, whether a hairline comes
/// before it, and what it does.
struct MoreItem {
    label: SharedString,
    icon: Icon,
    hint: Option<&'static str>,
    separated: bool,
    danger: bool,
    disabled: bool,
    act: super::Act,
}

impl MoreItem {
    fn new(
        label: impl Into<SharedString>,
        icon: Icon,
        act: impl Fn(&mut GitPane, &mut Context<GitPane>) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            icon,
            hint: None,
            separated: false,
            danger: false,
            disabled: false,
            act: std::rc::Rc::new(act),
        }
    }

    fn hint(mut self, hint: &'static str) -> Self {
        self.hint = Some(hint);
        self
    }

    fn separated(mut self, separated: bool) -> Self {
        self.separated = separated;
        self
    }

    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

/// A popover menu of what can be done with an entry, opened from its "More" button.
fn more(id: SharedString, view: &Entity<GitPane>, items: Vec<MoreItem>) -> impl IntoElement {
    let view = view.clone();
    ui::menu_button(
        id.clone(),
        Anchor::TopRight,
        ui::IconButton::new(SharedString::from(format!("{id}-more")), Icon::More, "More")
            .glyph(13.),
        move |mut popup, _, cx| {
            for line in &items {
                if line.separated {
                    popup = popup.separator();
                }
                let (view, act) = (view.clone(), line.act.clone());
                popup = popup.item(
                    ui::MenuRow::new(line.label.clone())
                        .icon(line.icon)
                        .danger(line.danger)
                        .when_some(line.hint, ui::MenuRow::hint)
                        .item(cx)
                        .disabled(line.disabled)
                        .on_click(move |_, _, cx| view.update(cx, |pane, cx| act(pane, cx))),
                );
            }
            popup
        },
    )
}

/// The Branches page: its list, then the button that makes a branch.
pub(super) fn branches_page(pane: &mut GitPane, cx: &mut Context<GitPane>) -> impl IntoElement {
    let t = Tokens::get(cx).clone();
    let view = cx.entity();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .when(pane.branches.entries.is_empty(), |page| {
            page.child(
                div()
                    .p(SPACE_5)
                    .text_size(crate::theme::ui_text(TEXT_XS))
                    .text_color(t.text_tertiary)
                    .child("This repository has no branches yet."),
            )
        })
        .child(
            list(pane.branch_rows.clone(), move |index, _, cx| {
                let cx = &*cx;
                let pane = view.read(cx);
                match pane.branches.entries.get(index) {
                    Some(entry) => branch_row(&view, pane, entry, index, cx),
                    None => div().into_any_element(),
                }
            })
            .flex_1()
            .px(SPACE_1H)
            .pt(SPACE_1),
        )
}

fn branch_row(
    view: &Entity<GitPane>,
    pane: &GitPane,
    entry: &BranchEntry,
    index: usize,
    cx: &App,
) -> AnyElement {
    let t = Tokens::get(cx).clone();
    let chosen = index == pane.branches.chosen;
    let key = SharedString::from(format!("branch:{index}"));
    let hovered = pane.hovered.as_ref() == Some(&key);
    let choosing = view.clone();
    let folds = matches!(
        entry,
        BranchEntry::Local { .. } | BranchEntry::Remote { .. }
    );
    let base = div()
        .id(key.clone())
        .relative()
        .w_full()
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .rounded(RADIUS_2)
        .cursor_pointer()
        .map(|row| {
            if chosen {
                row.bg(t.surface_chosen)
            } else {
                row.hover(move |row| row.bg(t.surface_hover))
            }
        })
        .on_hover(super::hover(view, key))
        .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
        .on_click(move |_, window, cx| {
            choosing.update(cx, |pane, cx| {
                window.focus(&pane.focus, cx);
                pane.choose_branch(index, cx);
                if folds {
                    pane.activate_branch(cx);
                }
            })
        });
    let group = |base: gpui_kit::Stateful<Div>,
                 icon: Icon,
                 name: String,
                 count: Option<usize>,
                 open: bool| {
        base.gap(SPACE_1)
            .pl(SPACE_0)
            .pr(SPACE_1)
            .child(motion::turn(
                format!("branch-chevron:{index}"),
                motion::fold_angle(open),
                motion::LIST_FOLD,
                ui::icon(Icon::Chevron, 13.).text_color(t.text_tertiary),
            ))
            .child(ui::icon(icon, 13.).text_color(t.text_tertiary))
            .child(
                ui::text(name, TEXT_XS, FontWeight::MEDIUM)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(t.text_primary),
            )
            .children(count.map(|count| {
                div()
                    .flex_none()
                    .min_w(px(18.))
                    .h(px(18.))
                    .px(px(5.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(t.surface_chosen)
                    .text_size(crate::theme::ui_text(TEXT_2XS))
                    .text_color(t.text_secondary)
                    .child(count.to_string())
            }))
    };
    let dot = |ink: Hsla| {
        div()
            .size(px(13.))
            .mx(px(1.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .child(div().size(px(7.)).rounded_full().bg(ink))
    };
    let name_and_tip = |name: &str, ink: Hsla, subject: &str| {
        let (folder, leaf) = match name.rfind('/') {
            Some(slash) => name.split_at(slash + 1),
            None => ("", name),
        };
        [
            div()
                .flex()
                .flex_shrink(1.)
                .min_w_0()
                .text_size(crate::theme::ui_text(TEXT_SM))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ink)
                .when(!folder.is_empty(), |name| {
                    name.child(
                        div()
                            .flex_none()
                            .text_color(t.text_tertiary)
                            .child(folder.to_owned()),
                    )
                })
                .child(div().min_w_0().truncate().child(leaf.to_owned())),
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_color(t.text_tertiary)
                .child(subject.to_owned()),
        ]
    };
    let age = |date: &str| {
        div()
            .flex_none()
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(t.text_tertiary)
            .child(date.to_owned())
    };
    match entry {
        BranchEntry::Local { count } => {
            let starting = view.clone();
            group(
                base,
                Icon::Laptop,
                "Local".into(),
                Some(*count),
                !pane.branches.local_folded,
            )
            .child(
                row_button("git-new-branch", Icon::Plus, 12., "New branch", cx).on_click(
                    move |_, _, cx| {
                        cx.stop_propagation();
                        starting.update(cx, |pane, cx| pane.new_branch(None, cx));
                    },
                ),
            )
            .into_any_element()
        }
        BranchEntry::Remote { remote, open } => {
            let fetched = remote.name.clone();
            let name = remote.name.clone();
            let mut row = group(base, Icon::Globe, remote.name.clone(), None, *open).mt(SPACE_1H);
            if hovered {
                let fetching = view.clone();
                row = row
                    .child(
                        row_button(
                            format!("fetch:{name}"),
                            Icon::Fetch,
                            12.,
                            format!("Fetch {name}"),
                            cx,
                        )
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            let fetched = fetched.clone();
                            fetching.update(cx, |pane, cx| pane.fetch_remote(Some(fetched), cx));
                        }),
                    )
                    .child(remote_more(view, remote));
            }
            row.into_any_element()
        }
        BranchEntry::Branch(branch) => {
            let status = match &pane.overview {
                Some(Ok(overview)) => Some(&overview.status),
                _ => None,
            };
            let (ahead, behind) = status.map_or((0, 0), |status| (status.ahead, status.behind));
            let mut row = base
                .gap(SPACE_2)
                .pl(px(18.))
                .pr(SPACE_1H)
                .text_size(crate::theme::ui_text(TEXT_XS_H))
                .whitespace_nowrap()
                .child(pane.author_face(&branch.author, &branch.author_email, 15., cx))
                .children(name_and_tip(
                    &branch.name,
                    if branch.current {
                        t.accent
                    } else if chosen {
                        t.text_primary
                    } else {
                        t.text_secondary
                    },
                    &branch.subject,
                ))
                .children(pull_chip(pane, &branch.name, index, cx));
            if branch.current && ahead > 0 {
                let pushing = view.clone();
                let label = format!("Push {ahead} commit{}", if ahead > 1 { "s" } else { "" });
                row = row.child(
                    gpui_kit::base::Button::new("git-sync-chip")
                        .accessibility_label(SharedString::from(label.clone()))
                        .tooltip(ui::tooltip(label))
                        .h(px(18.))
                        .px(SPACE_1)
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(SPACE_0)
                        .rounded(RADIUS_2)
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.warn)
                        .hover(move |style| style.bg(t.surface_hover))
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            pushing.update(cx, |pane, cx| pane.push(cx));
                        })
                        .child(format!("↑{ahead}")),
                );
            }
            if branch.current && behind > 0 {
                row = row.child(
                    div()
                        .flex_none()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.git_renamed)
                        .child(format!("↓{behind}")),
                );
            }
            if !hovered {
                row = row.child(age(&branch.date));
            }
            if hovered {
                let into = pane.current_branch();
                let into = if into.is_empty() {
                    "HEAD".to_owned()
                } else {
                    into
                };
                if !branch.current {
                    let (checking, merging) = (view.clone(), view.clone());
                    let (name, merged) = (branch.name.clone(), branch.name.clone());
                    row = row
                        .child(
                            row_button(
                                format!("checkout:{}", branch.name),
                                Icon::Checkout,
                                12.,
                                "Check out",
                                cx,
                            )
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let name = name.clone();
                                checking.update(cx, |pane, cx| pane.checkout(name, cx));
                            }),
                        )
                        .child(
                            row_button(
                                format!("merge:{}", branch.name),
                                Icon::Merge,
                                12.,
                                format!("Merge into {into}"),
                                cx,
                            )
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                let merged = merged.clone();
                                merging.update(cx, |pane, cx| pane.merge(merged, false, cx));
                            }),
                        );
                }
                row = row.child(branch_more(view, branch, &into));
            }
            row.into_any_element()
        }
        BranchEntry::RemoteBranch { remote, branch } => {
            let mut row = base
                .gap(SPACE_2)
                .pl(px(18.))
                .pr(SPACE_1H)
                .text_size(crate::theme::ui_text(TEXT_XS_H))
                .whitespace_nowrap()
                .child(dot(t.git_renamed))
                .children(name_and_tip(
                    &branch.name,
                    if chosen {
                        t.text_primary
                    } else {
                        t.text_secondary
                    },
                    branch.subject.as_deref().unwrap_or_default(),
                ))
                .children(
                    (remote == "origin")
                        .then(|| pull_chip(pane, &branch.name, index, cx))
                        .flatten(),
                );
            if branch.tracked_by.is_some() && !hovered {
                row = row.child(
                    div()
                        .flex_none()
                        .font_family(theme::MONO)
                        .text_size(crate::theme::ui_text(TEXT_2XS))
                        .text_color(t.text_tertiary)
                        .child("tracked"),
                );
            }
            if hovered {
                let checking = view.clone();
                let (remote_name, checked) = (remote.clone(), branch.clone());
                let label = match &branch.tracked_by {
                    Some(local) => format!("Check out {local}"),
                    None => "Check out as a local branch".to_owned(),
                };
                row = row
                    .child(
                        row_button(
                            format!("checkout:{}", branch.full_ref),
                            Icon::Checkout,
                            12.,
                            label,
                            cx,
                        )
                        .on_click(move |_, _, cx| {
                            cx.stop_propagation();
                            let (remote_name, checked) = (remote_name.clone(), checked.clone());
                            checking.update(cx, |pane, cx| {
                                pane.checkout_remote(remote_name, checked, cx)
                            });
                        }),
                    )
                    .child(remote_branch_more(
                        view,
                        remote,
                        branch,
                        &pane.current_branch(),
                    ));
            }
            row.into_any_element()
        }
    }
}

/// A branch's open pull request beside it, which opens it in the Git tab's pull requests.
fn pull_chip(pane: &GitPane, branch: &str, index: usize, cx: &App) -> Option<AnyElement> {
    let t = Tokens::get(cx).clone();
    let opened = pane.branches.pulls.get(branch)?;
    let number = opened.number;
    let ink = if opened.draft {
        t.text_tertiary
    } else {
        t.live
    };
    let bench = pane.bench.clone();
    let repo = pane.host_of()?.0.repo;
    Some(
        gpui_kit::base::Button::new(("branch-pull", index))
            .accessibility_label(SharedString::from(format!("Open pull request #{number}")))
            .tooltip(ui::tooltip(format!("#{number} {}", opened.title)))
            .h(px(20.))
            .px(SPACE_1H)
            .flex()
            .flex_none()
            .items_center()
            .gap(px(3.))
            .rounded(RADIUS_2)
            .border_1()
            .border_color(ink.opacity(0.32))
            .font_family(theme::MONO)
            .text_size(crate::theme::ui_text(TEXT_2XS))
            .text_color(ink)
            .hover(move |style| style.bg(ink.opacity(0.12)))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                bench
                    .update(cx, |bench, cx| {
                        bench.open_host_item(Area::Pulls, number, repo.clone(), cx)
                    })
                    .ok();
            })
            .child(ui::icon(Icon::PullRequest, 10.).text_color(ink))
            .child(format!("#{number}"))
            .into_any_element(),
    )
}

fn branch_more(view: &Entity<GitPane>, branch: &GitBranch, into: &str) -> impl IntoElement {
    let name = branch.name.clone();
    let mut items: Vec<MoreItem> = Vec::new();
    if !branch.current {
        let (checked, merged, squashed) = (name.clone(), name.clone(), name.clone());
        items.push(
            MoreItem::new("Check out", Icon::Checkout, move |pane, cx| {
                pane.checkout(checked.clone(), cx)
            })
            .hint("↵"),
        );
        items.push(
            MoreItem::new(
                format!("Merge into {into}"),
                Icon::Merge,
                move |pane, cx| pane.merge(merged.clone(), false, cx),
            )
            .hint("M"),
        );
        items.push(MoreItem::new(
            format!("Squash merge into {into}"),
            Icon::Merge,
            move |pane, cx| pane.merge(squashed.clone(), true, cx),
        ));
    }
    let (started, renamed, copied, deleted) =
        (name.clone(), branch.clone(), name.clone(), branch.clone());
    items.push(
        MoreItem::new("New branch from here", Icon::Plus, move |pane, cx| {
            pane.new_branch(Some(started.clone()), cx)
        })
        .hint("n")
        .separated(!branch.current),
    );
    items.push(
        MoreItem::new("Rename…", Icon::Pencil, move |pane, cx| {
            pane.rename_branch(&renamed, cx)
        })
        .hint("R"),
    );
    items.push(MoreItem::new("Copy name", Icon::Copy, move |pane, cx| {
        pane.copy(copied.clone(), copied.clone(), cx)
    }));
    if !branch.current {
        items.push(
            MoreItem::new("Delete…", Icon::Trash, move |pane, cx| {
                pane.delete_menu(&deleted, cx)
            })
            .hint("d")
            .separated(true)
            .danger(),
        );
    }
    more(
        SharedString::from(format!("branch-more:{name}")),
        view,
        items,
    )
}

fn remote_more(view: &Entity<GitPane>, remote: &GitRemote) -> impl IntoElement {
    let (fetched, edited, renamed, copied, removed) = (
        remote.name.clone(),
        remote.clone(),
        remote.clone(),
        remote.url.clone(),
        remote.clone(),
    );
    let what = format!("{} URL", remote.name);
    let items: Vec<MoreItem> = vec![
        MoreItem::new(
            format!("Fetch {}", remote.name),
            Icon::Fetch,
            move |pane, cx| pane.fetch_remote(Some(fetched.clone()), cx),
        )
        .hint("f"),
        MoreItem::new("Edit URL…", Icon::Pencil, move |pane, cx| {
            pane.edit_remote_url(&edited, cx)
        }),
        MoreItem::new("Rename…", Icon::Pencil, move |pane, cx| {
            pane.rename_remote(&renamed, cx)
        }),
        MoreItem::new("Copy URL", Icon::Copy, move |pane, cx| {
            pane.copy(copied.clone(), what.clone(), cx)
        }),
        MoreItem::new("Remove remote…", Icon::Trash, move |pane, cx| {
            pane.remove_remote(&removed, cx)
        })
        .separated(true)
        .danger(),
    ];
    more(
        SharedString::from(format!("remote-more:{}", remote.name)),
        view,
        items,
    )
}

fn remote_branch_more(
    view: &Entity<GitPane>,
    remote: &str,
    branch: &GitRemoteBranch,
    current: &str,
) -> impl IntoElement {
    let into = if current.is_empty() { "HEAD" } else { current };
    let (checked_remote, checked) = (remote.to_owned(), branch.clone());
    let (merged, upstream, copied) = (
        branch.full_ref.clone(),
        branch.clone(),
        branch.full_ref.clone(),
    );
    let (deleted_remote, deleted) = (remote.to_owned(), branch.clone());
    let check_out = match &branch.tracked_by {
        Some(local) => format!("Check out {local}"),
        None => "Check out as a local branch".to_owned(),
    };
    let items: Vec<MoreItem> = vec![
        MoreItem::new(check_out, Icon::Checkout, move |pane, cx| {
            pane.checkout_remote(checked_remote.clone(), checked.clone(), cx)
        })
        .hint("↵"),
        MoreItem::new(
            format!("Merge into {into}"),
            Icon::Merge,
            move |pane, cx| pane.merge(merged.clone(), false, cx),
        )
        .hint("M"),
        MoreItem::new(
            format!("Set as upstream of {into}"),
            Icon::Link,
            move |pane, cx| pane.set_upstream(&upstream, cx),
        )
        .hint("u")
        .disabled(current.is_empty()),
        MoreItem::new("Copy name", Icon::Copy, move |pane, cx| {
            pane.copy(copied.clone(), copied.clone(), cx)
        }),
        MoreItem::new(
            format!("Delete on {remote}…"),
            Icon::Trash,
            move |pane, cx| pane.delete_remote_branch(deleted_remote.clone(), &deleted, cx),
        )
        .separated(true)
        .danger(),
    ];
    more(
        SharedString::from(format!("remote-branch-more:{}", branch.full_ref)),
        view,
        items,
    )
}

/// The card heading a branch's review: its name and upstream, what the checked-out one
/// has to push and pull, and what can be done with it.
pub(super) fn branch_card(
    pane: &GitPane,
    entry: &BranchEntry,
    cx: &mut Context<GitPane>,
) -> Option<Div> {
    let t = Tokens::get(cx).clone();
    let fact = |label: &'static str, value: String, ink: Hsla| {
        div()
            .flex()
            .items_center()
            .min_h(px(22.))
            .gap(SPACE_3)
            .child(
                div()
                    .w(px(64.))
                    .flex_none()
                    .text_color(t.text_tertiary)
                    .child(label),
            )
            .child(div().min_w_0().text_color(ink).child(value))
    };
    let (title, facts): (String, Vec<Div>) = match entry {
        BranchEntry::Branch(branch) => {
            let mut facts = vec![fact(
                "Upstream",
                branch.upstream.clone().unwrap_or_else(|| "none".into()),
                t.text_secondary,
            )];
            if branch.current {
                let (ahead, behind) = match &pane.overview {
                    Some(Ok(overview)) => (overview.status.ahead, overview.status.behind),
                    _ => (0, 0),
                };
                let plural = |count: i32| if count == 1 { "" } else { "s" };
                facts.push(fact(
                    "To push",
                    format!("{ahead} commit{}", plural(ahead)),
                    if ahead > 0 { t.live } else { t.text_secondary },
                ));
                facts.push(fact(
                    "To pull",
                    format!("{behind} commit{}", plural(behind)),
                    if behind > 0 { t.warn } else { t.text_secondary },
                ));
                let changed = pane.files().len();
                facts.push(fact(
                    "Changes",
                    format!(
                        "{changed} uncommitted file{}",
                        if changed == 1 { "" } else { "s" }
                    ),
                    t.text_secondary,
                ));
            }
            (branch.name.clone(), facts)
        }
        BranchEntry::Remote { remote, .. } => (
            remote.name.clone(),
            vec![fact("URL", remote.url.clone(), t.text_secondary)],
        ),
        BranchEntry::RemoteBranch { branch, .. } => (
            branch.full_ref.clone(),
            vec![fact(
                "Local",
                branch
                    .tracked_by
                    .clone()
                    .unwrap_or_else(|| "no local branch".into()),
                t.text_secondary,
            )],
        ),
        BranchEntry::Local { .. } => return None,
    };
    let current = matches!(entry, BranchEntry::Branch(branch) if branch.current);
    Some(
        ui::panel(cx)
            .flex_none()
            .child(
                div()
                    .px(SPACE_4)
                    .pt(SPACE_3H)
                    .pb(SPACE_3)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(SPACE_2)
                            .font_family(theme::MONO)
                            .text_size(crate::theme::ui_text(TEXT_LG))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .child(title)
                            .when(current, |title| {
                                title.child(
                                    div()
                                        .h(px(18.))
                                        .px(SPACE_1H)
                                        .flex()
                                        .items_center()
                                        .border_1()
                                        .border_color(t.live.opacity(0.35))
                                        .rounded(px(4.))
                                        .text_size(crate::theme::ui_text(TEXT_3XS))
                                        .font_weight(FontWeight::NORMAL)
                                        .text_color(t.live)
                                        .child("checked out"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .mt(SPACE_3)
                            .flex()
                            .flex_col()
                            .gap(SPACE_1)
                            .text_size(crate::theme::ui_text(TEXT_2XS))
                            .children(facts),
                    ),
            )
            .child(card_foot(pane, entry, cx)),
    )
}

/// The buttons along the foot of a branch's card.
fn card_foot(pane: &GitPane, entry: &BranchEntry, cx: &mut Context<GitPane>) -> Div {
    let t = Tokens::get(cx).clone();
    let into = pane.current_branch();
    let into = if into.is_empty() {
        "HEAD".to_owned()
    } else {
        into
    };
    let button = |id: String,
                  icon: Icon,
                  label: String,
                  ink: Option<Hsla>,
                  primary: bool,
                  act: super::Act,
                  cx: &mut Context<GitPane>| {
        gpui_kit::base::Button::new(SharedString::from(id))
            .accessibility_label(SharedString::from(label.clone()))
            .h(px(26.))
            .px(SPACE_2)
            .flex()
            .items_center()
            .gap(SPACE_1H)
            .rounded(RADIUS_2)
            .text_size(crate::theme::ui_text(TEXT_XS))
            .map(|button| {
                if primary {
                    button.bg(t.accent).text_color(t.on_accent)
                } else {
                    let ink = ink.unwrap_or(t.text_secondary);
                    button.text_color(ink).hover(move |style| {
                        style.bg(t.surface_hover).text_color(if ink == t.danger {
                            ink
                        } else {
                            t.text_primary
                        })
                    })
                }
            })
            .on_click(cx.listener(move |pane, _, _, cx| act(pane, cx)))
            .child(ui::icon(icon, 12.).text_color(if primary {
                t.on_accent
            } else {
                ink.unwrap_or(t.text_tertiary)
            }))
            .child(label)
    };
    let group = || {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(SPACE_0)
            .min_w_0()
    };
    let foot = div()
        .flex()
        .items_center()
        .justify_between()
        .px(SPACE_2)
        .py(SPACE_1H)
        .border_t_1()
        .border_color(t.border);
    match entry {
        BranchEntry::Branch(branch) if branch.current => {
            let upstream = branch.upstream.is_some();
            let ahead = match &pane.overview {
                Some(Ok(overview)) => overview.status.ahead,
                _ => 0,
            };
            let (started, renamed) = (branch.name.clone(), branch.clone());
            foot.child(
                group()
                    .child(button(
                        "card-new".into(),
                        Icon::Plus,
                        "New branch from here".into(),
                        None,
                        false,
                        std::rc::Rc::new(move |pane, cx| {
                            pane.new_branch(Some(started.clone()), cx)
                        }),
                        cx,
                    ))
                    .child(button(
                        "card-rename".into(),
                        Icon::Pencil,
                        "Rename".into(),
                        None,
                        false,
                        std::rc::Rc::new(move |pane, cx| pane.rename_branch(&renamed, cx)),
                        cx,
                    )),
            )
            .child(
                group()
                    .child(button(
                        "card-pull".into(),
                        Icon::Pull,
                        "Pull".into(),
                        None,
                        false,
                        std::rc::Rc::new(|pane, cx| pane.pull(cx)),
                        cx,
                    ))
                    .child(button(
                        "card-push".into(),
                        Icon::Push,
                        if upstream { "Push" } else { "Publish" }.into(),
                        None,
                        ahead > 0 || !upstream,
                        std::rc::Rc::new(|pane, cx| pane.push(cx)),
                        cx,
                    )),
            )
        }
        BranchEntry::Branch(branch) => {
            let (merged, squashed, deleted, checked) = (
                branch.name.clone(),
                branch.name.clone(),
                branch.clone(),
                branch.name.clone(),
            );
            foot.child(
                group()
                    .child(button(
                        "card-merge".into(),
                        Icon::Merge,
                        format!("Merge into {into}"),
                        None,
                        false,
                        std::rc::Rc::new(move |pane, cx| pane.merge(merged.clone(), false, cx)),
                        cx,
                    ))
                    .child(button(
                        "card-squash".into(),
                        Icon::Merge,
                        "Squash merge".into(),
                        None,
                        false,
                        std::rc::Rc::new(move |pane, cx| pane.merge(squashed.clone(), true, cx)),
                        cx,
                    ))
                    .child(button(
                        "card-delete".into(),
                        Icon::Trash,
                        "Delete".into(),
                        Some(t.danger),
                        false,
                        std::rc::Rc::new(move |pane, cx| pane.delete_menu(&deleted, cx)),
                        cx,
                    )),
            )
            .child(button(
                "card-checkout".into(),
                Icon::Checkout,
                "Check out".into(),
                None,
                true,
                std::rc::Rc::new(move |pane, cx| pane.checkout(checked.clone(), cx)),
                cx,
            ))
        }
        BranchEntry::RemoteBranch { remote, branch } => {
            let (merged, upstream, deleted_remote, deleted, checked_remote, checked) = (
                branch.full_ref.clone(),
                branch.clone(),
                remote.clone(),
                branch.clone(),
                remote.clone(),
                branch.clone(),
            );
            let label = match &branch.tracked_by {
                Some(local) => format!("Check out {local}"),
                None => "Check out".to_owned(),
            };
            foot.child(
                group()
                    .child(button(
                        "card-merge".into(),
                        Icon::Merge,
                        format!("Merge into {into}"),
                        None,
                        false,
                        std::rc::Rc::new(move |pane, cx| pane.merge_menu(merged.clone(), cx)),
                        cx,
                    ))
                    .child(button(
                        "card-upstream".into(),
                        Icon::Link,
                        "Set as upstream".into(),
                        None,
                        false,
                        std::rc::Rc::new(move |pane, cx| pane.set_upstream(&upstream, cx)),
                        cx,
                    ))
                    .child(button(
                        "card-delete".into(),
                        Icon::Trash,
                        format!("Delete on {remote}"),
                        Some(t.danger),
                        false,
                        std::rc::Rc::new(move |pane, cx| {
                            pane.delete_remote_branch(deleted_remote.clone(), &deleted, cx)
                        }),
                        cx,
                    )),
            )
            .child(button(
                "card-checkout".into(),
                Icon::Checkout,
                label,
                None,
                true,
                std::rc::Rc::new(move |pane, cx| {
                    pane.checkout_remote(checked_remote.clone(), checked.clone(), cx)
                }),
                cx,
            ))
        }
        BranchEntry::Remote { remote, .. } => {
            let (fetched, copied) = (remote.name.clone(), remote.url.clone());
            let what = format!("{} URL", remote.name);
            foot.child(
                group()
                    .child(button(
                        "card-fetch".into(),
                        Icon::Fetch,
                        format!("Fetch {}", remote.name),
                        None,
                        false,
                        std::rc::Rc::new(move |pane, cx| {
                            pane.fetch_remote(Some(fetched.clone()), cx)
                        }),
                        cx,
                    ))
                    .child(button(
                        "card-copy".into(),
                        Icon::Copy,
                        "Copy URL".into(),
                        None,
                        false,
                        std::rc::Rc::new(move |pane, cx| {
                            pane.copy(copied.clone(), what.clone(), cx)
                        }),
                        cx,
                    )),
            )
        }
        BranchEntry::Local { .. } => foot,
    }
}
