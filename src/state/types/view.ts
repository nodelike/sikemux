export interface EditorPaneView {
    openTabs: string[];
    activePath: string | null;
    treeWidth: number;
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
}

export const DEFAULT_GIT_VIEW: GitPaneView = {
    panel: "files",
    selected: { status: 0, files: 0, branches: 0, remotes: 0, commits: 0, stashes: 0 },
    remoteDrill: null,
    remoteBranchSelected: {},
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

export type EcsLevel =
    | { kind: "clusters" }
    | { kind: "services"; cluster: string }
    | {
          kind: "service";
          cluster: string;
          service: string;
          tab: "logs" | "tasks";
          taskFilter?: { taskId: string; stream: string };
      };

export type RundeckLevel =
    | { kind: "matrix" }
    | { kind: "service"; env: string; project: string; service: string; jobId: string; repoPath?: string }
    | {
          kind: "deploy";
          env: string;
          project: string;
          service: string;
          jobId: string;
          branch: string;
          repoPath?: string;
      }
    | { kind: "execution"; executionId: number; service: string; project: string; env?: string; jobId?: string; repoPath?: string };

export interface RundeckView {
    stack: RundeckLevel[];
}

export type BrunoReqTab = "params" | "body" | "headers" | "auth" | "vars" | "script" | "docs";
export type BrunoResTab = "body" | "headers" | "timeline" | "tests";

/** Ephemeral (non-persisted) per-session Bruno UI state, keyed by session id. */
export interface BrunoView {
    /** ordered list of open request tabs (file paths) */
    openPaths: string[];
    activeRequestPath: string | null;
    reqTab: BrunoReqTab;
    resTab: BrunoResTab;
    /** request pane width in the request/response split, as a percent */
    reqPanePct: number;
    secretsOpen: boolean;
}

export const DEFAULT_BRUNO_VIEW: BrunoView = {
    openPaths: [],
    activeRequestPath: null,
    reqTab: "params",
    resTab: "body",
    reqPanePct: 50,
    secretsOpen: false,
};
