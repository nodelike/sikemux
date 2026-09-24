export interface EditorPaneView {
    openTabs: string[];
    activePath: string | null;
}

/** A page a browser pane can open again, with the title to label it until it loads. */
export interface BrowserPaneTab {
    url: string;
    title: string;
}

/** What a browser pane needs to come back: whose browser it is, and what was in it. */
export interface BrowserPaneView {
    agentId: string;
    tabs: BrowserPaneTab[];
    activeIndex: number;
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

export type GitPanel = "status" | "files" | "branches" | "remotes" | "commits" | "stashes";

export interface GitPaneView {
    panel: GitPanel;
    selected: Record<GitPanel, number>;
    remoteDrill: string | null;
    remoteBranchSelected: Record<string, number>;
    /** A repository found inside the project folder, when the folder is not one itself. */
    repo: string | null;
}

export const DEFAULT_GIT_VIEW: GitPaneView = {
    panel: "files",
    selected: { status: 0, files: 0, branches: 0, remotes: 0, commits: 0, stashes: 0 },
    remoteDrill: null,
    remoteBranchSelected: {},
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
