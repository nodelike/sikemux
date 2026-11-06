//! The pane's keys, and the cheatsheet that lists them.

use super::*;

impl GitPane {
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
            "F" => self.fetch(cx),
            "P" => self.push(cx),
            "p" => self.pull(cx),
            "r" if !history => self.refresh(cx),
            _ if self.page == Page::Branches && self.branch_key(typed, cx) => {}
            _ if self.page == Page::Branches => return,
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
pub(crate) const KEYS: [ui::Section; 4] = [
    (
        "Global",
        &[
            ("1 2 3", "Changes, History, Branches"),
            ("h", "switch between Changes and History"),
            ("?", "open this cheatsheet"),
            ("@", "toggle command log"),
            ("/", "filter the list"),
            ("r", "refresh repo state"),
            ("F", "fetch all remotes"),
            ("P / p", "push / pull"),
            ("^P", "open pull-request page"),
            ("esc", "close modal / clear filter or range"),
        ],
    ),
    (
        "Changes",
        &[
            ("space", "stage / unstage selected (or range)"),
            ("v", "toggle range select"),
            ("a", "toggle stage all"),
            ("c", "focus commit message"),
            ("C", "commit the typed message"),
            ("g", "generate commit message (AI)"),
            ("⌘⏎", "commit (from message box)"),
            ("d", "discard menu"),
            ("s", "stash menu"),
        ],
    ),
    (
        "History",
        &[
            ("enter / space", "actions menu"),
            ("b", "create branch from commit"),
            ("r", "reset to commit menu"),
            ("v", "revert commit"),
        ],
    ),
    (
        "Branches",
        &[
            (
                "enter / space",
                "checkout (a remote branch gets a local one)",
            ),
            ("n / N", "new branch (from selected / from HEAD)"),
            ("M", "merge menu"),
            ("d", "delete menu"),
            ("R", "rename branch"),
            ("c", "checkout by name"),
            ("f", "fetch the selected remote"),
            ("u", "set remote branch as upstream"),
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
}
