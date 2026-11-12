//! The pane's keys, and the cheatsheet that lists them.

use super::branches::BranchEntry;
use super::*;

impl GitPane {
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

    pub(crate) fn key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keys = &event.keystroke;
        if self.draft.focus_handle(cx).is_focused(window) || self.asking.is_some() {
            return;
        }
        let modifiers = keys.modifiers;
        if modifiers.alt || modifiers.platform {
            return;
        }
        if modifiers.control {
            if keys.key == "p" {
                self.pull_request(cx);
                cx.stop_propagation();
            }
            return;
        }
        if self.busy.is_some() {
            return;
        }
        let typed = match keys.key.as_str() {
            named @ ("enter" | "space" | "up" | "down" | "escape") => named,
            key => keys.key_char.as_deref().unwrap_or(key),
        };
        let history = self.page == Page::History;
        match typed {
            "/" => self.find(window, cx),
            "?" => {
                self.asking = Some(Asking::Keys);
                cx.notify();
            }
            "@" => {
                self.commands_open = !self.commands_open;
                cx.notify();
            }
            "escape" if self.range.is_some() => {
                self.range = None;
                cx.notify();
            }
            "escape" => {
                if !self.unfind(window, cx) {
                    return;
                }
            }
            "1" => self.turn(Page::Changes, cx),
            "2" => self.turn(Page::History, cx),
            "3" => self.turn(Page::Branches, cx),
            "4" => self.turn(Page::Pulls, cx),
            "F" => self.fetch(cx),
            "P" => self.push(cx),
            "p" => self.pull(cx),
            "r" if !history => self.refresh(cx),
            _ if self.page == Page::Branches && self.branch_key(typed, cx) => {}
            _ if self.page == Page::Branches => return,
            "j" | "down" if self.page == Page::Pulls => self.step_pull(1, cx),
            "k" | "up" if self.page == Page::Pulls => self.step_pull(-1, cx),
            " " | "space" | "enter" if self.page == Page::Pulls => self.open_pull(cx),
            _ if self.page == Page::Pulls => return,
            "h" if history => self.turn(Page::Changes, cx),
            "h" => self.turn(Page::History, cx),
            "j" | "down" if history => self.step_commit(1, cx),
            "k" | "up" if history => self.step_commit(-1, cx),
            " " | "space" | "enter" if history => self.commit_menu(cx),
            "b" if history => self.branch_from_commit(cx),
            "r" if history => self.reset_menu(cx),
            "v" if history => self.revert_commit(cx),
            _ if history => return,
            "j" => self.step(1, cx),
            "k" => self.step(-1, cx),
            "down" => self.step(1, cx),
            "up" => self.step(-1, cx),
            " " | "space" => self.toggle_stage(cx),
            "a" if self.files().iter().any(GitFile::has_unstaged) => self.stage_all(cx),
            "a" => self.unstage_all(cx),
            "c" => {
                let draft = self.draft.clone();
                draft.update(cx, |draft, cx| draft.focus(window, cx));
            }
            "C" => self.commit(cx),
            "g" => self.generate(cx),
            "d" => self.discard_menu(cx),
            "v" => {
                self.range = match self.range {
                    Some(_) => None,
                    None => Some(self.selected),
                };
                cx.notify();
            }
            "s" => self.stash_menu(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

/// The pane's keys, by page, as its cheatsheet lists them.
pub(crate) const KEYS: [ui::Section; 5] = [
    (
        n!("Global"),
        &[
            ("1 2 3 4", n!("Changes, History, Branches, Pull requests")),
            ("h", n!("switch between Changes and History")),
            ("?", n!("open this cheatsheet")),
            ("@", n!("toggle command log")),
            ("/", n!("filter the list")),
            ("r", n!("refresh repo state")),
            ("F", n!("fetch all remotes")),
            ("P / p", n!("push / pull")),
            ("^P", n!("open pull-request page")),
            ("esc", n!("close modal / clear filter or range")),
        ],
    ),
    (
        n!("Changes"),
        &[
            ("space", n!("stage / unstage selected (or range)")),
            ("v", n!("toggle range select")),
            ("a", n!("toggle stage all")),
            ("c", n!("focus commit message")),
            ("C", n!("commit the typed message")),
            ("g", n!("generate commit message (AI)")),
            ("⌘⏎", n!("commit (from message box)")),
            ("d", n!("discard menu")),
            ("s", n!("stash menu")),
        ],
    ),
    (
        n!("History"),
        &[
            ("enter / space", n!("actions menu")),
            ("b", n!("create branch from commit")),
            ("r", n!("reset to commit menu")),
            ("v", n!("revert commit")),
        ],
    ),
    (
        n!("Branches"),
        &[
            (
                "enter / space",
                n!("checkout (a remote branch gets a local one)"),
            ),
            ("n / N", n!("new branch (from selected / from HEAD)")),
            ("M", n!("merge menu")),
            ("d", n!("delete menu")),
            ("R", n!("rename branch")),
            ("c", n!("checkout by name")),
            ("f", n!("fetch the selected remote")),
            ("u", n!("set remote branch as upstream")),
        ],
    ),
    (
        n!("Pull requests"),
        &[
            ("enter / space", n!("open its slice, or start one")),
            ("j / k", n!("move between pull requests")),
        ],
    ),
];

/// How long typing rests before the commit message is kept as the draft.
pub(crate) const DRAFT_PAUSE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Changes,
    History,
    Branches,
    Pulls,
}
