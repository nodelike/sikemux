import type { Section } from "../../codehost/state";
export interface EditorPaneView {
    openTabs: string[];
    activePath: string | null;
    /** A view split beside other work that shows one file, rather than an editor holding the tab's files. */
    single?: true;
    /** The tab a single click in the file tree opened. The next such click reuses it, until it is kept. */
    preview?: string;
}

/** A page the desk can open again, with the title to label it until it loads. */
export interface DeskBrowserTab {
    url: string;
    title: string;
}

/** What a desk needs to come back: whose desk it is, and the pages and files that were on it. */
export interface DeskView {
    agentId: string;
    tabs: DeskBrowserTab[];
    activeIndex: number;
    files: string[];
}

/** A task terminal an agent started, shown on its desk and bound to the task's process by `id`. */
export interface DeskTerminal {
    id: string;
    terminalKey: string;
    label: string;
    cwd: string;
}

/** The iOS Simulator on a desk, and the device it shows once one is picked. */
export interface DeskSimulator {
    id: string;
    udid: string | null;
    deviceName: string | null;
}

/**
 * Keys in `order` and `active` name what they point at: `browser:<tab id>`,
 * `file:<path>`, `terminal:<id>` or `simulator:<id>`. The browser has one page on screen at a
 * time, so `active` is just `browser` when a page is showing.
 */
export interface Desk {
    order: string[];
    active: string | null;
    terminals: DeskTerminal[];
    simulators: DeskSimulator[];
    /** The latest file the agent or the person asked to see, and where in it. */
    reveal: DeskReveal | null;
}

export interface DeskReveal {
    path: string;
    line?: number;
    character?: number;
    seq: number;
}

/** A path handed to the running app by the `sikemux` command-line client. */
export interface CliOpenTarget {
    id: string;
    kind: "file" | "directory";
    path: string;
    projectRoot: string;
    /** Zero-based editor position. */
    line?: number;
    /** Zero-based editor position. */
    column?: number;
}

export interface CliOpenRequest {
    id: string;
    cwd: string;
    wait: boolean;
    targets: CliOpenTarget[];
}

export interface CliFrontendRequest {
    request: CliOpenRequest;
}

/** Runtime-only work claimed by an editor pane from the CLI bridge. */
export interface CliPendingEditorOpen extends CliOpenTarget {
    requestId: string;
}

export interface CliOpenResult {
    requestId: string;
    targetId: string;
    paneId: string | null;
    path: string;
    error: string | null;
}

/** Which list the git pane is on: the changed files or the history under them (both in Changes), or Branches. */
export type GitPanel = "files" | "commits" | "branches";

/** The local workbench, or one of the code host's sections. */
export type GitArea = "local" | Section;

export interface GitPaneView {
    area: GitArea;
    panel: GitPanel;
    selected: Record<GitPanel, number>;
    /** The remote whose branches are listed under the local ones in the Branches tab. */
    openRemote: string | null;
    /** The left column's width in pixels once someone has dragged it; null keeps the default. */
    leftWidth: number | null;
    /** Whether the history under the changed files is open. */
    historyOpen: boolean;
    /** The open history's height in pixels once someone has dragged it; null shares the column evenly. */
    historyHeight: number | null;
    /** A repository found inside the project folder, when the folder is not one itself. */
    repo: string | null;
}

export const DEFAULT_GIT_VIEW: GitPaneView = {
    area: "local",
    panel: "files",
    selected: { files: 0, commits: 0, branches: 0 },
    openRemote: "origin",
    leftWidth: null,
    historyOpen: false,
    historyHeight: null,
    repo: null,
};

export interface GlobalSearchView {
    query: string;
    replace: string;
    replaceOpen: boolean;
    options: {
        caseSensitive: boolean;
        wholeWord: boolean;
        isRegex: boolean;
        include: string;
        exclude: string;
    };
    collapsed: Record<string, boolean>;
    selected: { path: string; matchIndex: number } | null;
}

export type KeyModifier = "Alt" | "Control" | "Meta" | "Shift";

export interface SessionSwitcherView {
    sessionIds: string[];
    selectedSessionId: string;
    releaseModifier: KeyModifier;
}

export const DEFAULT_GLOBAL_SEARCH_VIEW: GlobalSearchView = {
    query: "",
    replace: "",
    replaceOpen: false,
    options: {
        caseSensitive: false,
        wholeWord: false,
        isRegex: false,
        include: "",
        exclude: "",
    },
    collapsed: {},
    selected: null,
};
