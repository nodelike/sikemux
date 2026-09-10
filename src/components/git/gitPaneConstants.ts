import type { GitCheatsheetSection } from "../../state/gitTypes";
import { PRIMARY_SHORTCUT } from "../../lib/platform";
import type { GitPanel } from "../../state/types";
import { helpRows } from "./gitPaneLogic";
import type { GitAiProvider } from "./gitPaneTypes";

export const AI_PROVIDER_LABEL: Record<GitAiProvider, string> = {
    hermes: "Hermes",
    codex: "Codex",
    claude: "Claude",
};

export const AI_MODELS: Record<GitAiProvider, string[]> = {
    hermes: ["openai/gpt-5.5", "openai/gpt-5.1", "anthropic/claude-sonnet-4.6", "anthropic/claude-opus-4.1", "google/gemini-2.5-pro"],
    codex: ["gpt-5.5", "gpt-5.1-codex-max", "gpt-5.1-codex", "gpt-5.1-codex-mini", "gpt-5.1", "gpt-5"],
    claude: ["sonnet", "opus", "haiku", "opusplan", "sonnet[1m]", "default"],
};

export const DEFAULT_AI_PROVIDER: GitAiProvider = "hermes";
export const AI_PROVIDER_STORAGE = "sikemux.git.ai.provider";
export const AI_MODEL_STORAGE = "sikemux.git.ai.model";

/** Height of the Git pane's built-in terminal, as a percent of the right column. */

export const defaultAiModel = (provider: GitAiProvider): string => AI_MODELS[provider][0];

export const GIT_PANEL_ORDER: GitPanel[] = ["files", "branches", "commits", "remotes", "stashes"];
export const GIT_PANEL_BY_KEY: Partial<Record<string, GitPanel>> = { "2": "files", "3": "branches", "4": "commits", "5": "remotes", "6": "stashes" };

export const GIT_HELP: GitCheatsheetSection[] = [
    {
        title: "Global",
        rows: helpRows(
            ["2..6", "switch panel"],
            ["?", "open this cheatsheet"],
            ["@", "toggle command log"],
            ["/", "filter current panel"],
            ["v", "toggle range select"],
            ["r", "refresh repo state"],
            ["P / p", "push / pull"],
            ["^P", "open pull-request page"],
            ["esc", "close modal / clear filter or range"],
        ),
    },
    {
        title: "Files",
        rows: helpRows(
            ["space", "stage / unstage selected (or range)"],
            ["a", "toggle stage all"],
            ["c", "focus commit message box"],
            ["C", "commit the typed message"],
            ["g", "generate commit message (AI)"],
            [`${PRIMARY_SHORTCUT}⏎`, "commit (from message box)"],
            ["d", "discard menu"],
            ["s", "stash menu"],
        ),
    },
    {
        title: "Branches",
        rows: helpRows(
            ["enter / space", "checkout"],
            ["n / N", "new branch (from selected / from HEAD)"],
            ["M", "merge menu"],
            ["d", "delete menu"],
            ["R", "rename branch"],
            ["c", "checkout by name"],
        ),
    },
    {
        title: "Remotes (list)",
        rows: helpRows(
            ["enter", "drill into remote's branches"],
            ["n", "add remote"],
            ["f", "fetch this remote"],
            ["F", "fetch all remotes"],
            ["e", "edit url"],
            ["r", "rename"],
            ["d", "delete"],
            ["...", "any of the above also openable via menu"],
        ),
    },
    {
        title: "Remotes (drilled)",
        rows: helpRows(
            ["esc", "back to remotes list"],
            ["space / enter", "checkout (creates tracking branch)"],
            ["M", "merge into HEAD"],
            ["u", "set as upstream of current branch"],
            ["d", "delete remote branch"],
            ["f / F", "fetch (this remote / all)"],
        ),
    },
    {
        title: "Commits",
        rows: helpRows(["enter / space", "actions menu"], ["b", "create branch from commit"], ["r", "reset to commit menu"], ["v", "revert commit"]),
    },
    {
        title: "Stashes",
        rows: helpRows(
            ["enter", "actions menu"],
            ["space / a", "apply stash"],
            ["p", "pop stash"],
            ["b", "branch from stash"],
            ["r", "rename stash"],
            ["d", "drop stash"],
        ),
    },
];
