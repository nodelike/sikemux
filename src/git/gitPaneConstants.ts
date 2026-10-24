import type { GitCheatsheetSection } from "../state/gitTypes";
import { PRIMARY_SHORTCUT } from "../lib/platform";
import type { GitPanel } from "../state/types";
import { helpRows } from "./gitPaneLogic";
import type { AgentType } from "../agents/catalog";

export const DEFAULT_AI_PROVIDER: AgentType = "hermes";

/** Height of the Git pane's built-in terminal, as a percent of the right column. */

export const GIT_PANEL_BY_KEY: Partial<Record<string, GitPanel>> = { "1": "files", "2": "commits", "3": "branches" };

export const GIT_HELP: GitCheatsheetSection[] = [
    {
        title: "Global",
        rows: helpRows(
            ["1 2 3 4", "Changes, History, Branches, Inbox"],
            ["h", "switch between Changes and History"],
            ["?", "open this cheatsheet"],
            ["@", "toggle command log"],
            ["/", "filter the list"],
            ["r", "refresh repo state"],
            ["F", "fetch all remotes"],
            ["P / p", "push / pull"],
            ["^P", "open pull-request page"],
            ["esc", "close modal / clear filter or range"],
        ),
    },
    {
        title: "Changes",
        rows: helpRows(
            ["space", "stage / unstage selected (or range)"],
            ["v", "toggle range select"],
            ["a", "toggle stage all"],
            ["c", "focus commit message"],
            ["C", "commit the typed message"],
            ["g", "generate commit message (AI)"],
            [`${PRIMARY_SHORTCUT}⏎`, "commit (from message box)"],
            ["d", "discard menu"],
            ["s", "stash menu"],
        ),
    },
    {
        title: "History",
        rows: helpRows(["enter / space", "actions menu"], ["b", "create branch from commit"], ["r", "reset to commit menu"], ["v", "revert commit"]),
    },
    {
        title: "Branches",
        rows: helpRows(
            ["enter / space", "checkout (a remote branch gets a local one)"],
            ["n / N", "new branch (from selected / from HEAD)"],
            ["M", "merge menu"],
            ["d", "delete menu"],
            ["R", "rename branch"],
            ["c", "checkout by name"],
            ["f", "fetch the selected remote"],
            ["u", "set remote branch as upstream"],
        ),
    },
];
