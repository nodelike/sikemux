import type { PluginKind } from "../../plugins/kinds";

/**
 * How a split arranges its children.
 *
 * `row` and `column` tile them side by side. `stack` puts them in the same
 * place and shows one at a time, which is what a tab strip is — the children
 * are tabs, and the active pane decides which is on top.
 */
export type SplitDir = "row" | "column" | "stack";
export type CorePaneKind = "terminal" | "editor" | "git" | "diff" | "search" | "agent" | "browser";
export type PaneKind = CorePaneKind | PluginKind;

export interface PaneNode {
    type: "pane";
    id: string;
    cwd: string;
    kind: PaneKind;
    title: string;
    startup?: string;
    /** Runtime-only marker: this pane borrows a process owned outside its renderer. */
    externalPty?: true;
    /** Runtime-only stable task identity used to reuse its presentation window. */
    taskTerminalKey?: string;
}

export interface SplitNode {
    type: "split";
    id: string;
    dir: SplitDir;
    children: LayoutNode[];
    sizes: number[];
}

export type LayoutNode = PaneNode | SplitNode;

export type SessionKind = "project" | "command" | "ssh" | PluginKind;

export type WindowRole = "term" | "files" | "git" | "diff" | "search" | "ssh-config" | "named" | "agent" | PluginKind;

export interface Window {
    id: string;
    name: string;
    role: WindowRole;
    root: LayoutNode;
    activePaneId: string;
    fixed?: boolean;
    /** Runtime-only windows are deliberately omitted from persistence. */
    transient?: true;
}

export type AgentType = "claude" | "codex" | "hermes" | "pi" | "opencode" | "omp" | "grok";

/** The permission boundary applied when starting an agent process. */
export type AgentPermissionMode = "read-only" | "workspace-write" | "full-access" | "bypass";

/** Provider-neutral reasoning levels; unsupported levels are normalized per CLI. */
export type AgentEffort = "off" | "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";

/** Supported profile backends. Gemini is profile-ready ahead of a dedicated agent tab. */
export type AgentProvider = "claude" | "codex" | "gemini";

/**
 * Durable, non-secret configuration for an agent executable.
 * `environmentKeys` stores names only; credential values remain in the user's
 * shell, provider config, or OS credential store.
 */
export interface ProviderProfile {
    id: string;
    name: string;
    provider: AgentProvider;
    accent: string;
    executablePath?: string;
    configPath?: string;
    environmentKeys?: string[];
}

export type ProviderProfileSelection = Partial<Record<AgentType, string>>;

export const DEFAULT_PROVIDER_PROFILES: readonly ProviderProfile[] = [
    { id: "builtin-claude", name: "Claude", provider: "claude", accent: "#d97757" },
    { id: "builtin-codex", name: "Codex", provider: "codex", accent: "#7a9dff" },
    { id: "builtin-gemini", name: "Gemini", provider: "gemini", accent: "#4285f4" },
];

export const DEFAULT_PROVIDER_PROFILE_SELECTION: Readonly<ProviderProfileSelection> = {
    claude: "builtin-claude",
    codex: "builtin-codex",
};

export interface Agent {
    id: string;
    type: AgentType;
    title: string;
    startup: string;
    /** Runtime-only structured launch; avoids shell parsing and argv prompt exposure. */
    directCommand?: PtyDirectCommand;
    resumeId?: string;
    createdAt?: number;
    /** Explicit launch boundary. Absent on legacy in-memory records. */
    permissionMode?: AgentPermissionMode;
    /** Non-secret provider profile selected for this launch. */
    profileId?: string;
    /** Health-checked executable used for this launch. */
    executablePath?: string;
    /** Effective launch directory. */
    cwd?: string;
    /** Optional provider model override selected at launch. */
    model?: string;
    /** Optional provider reasoning-effort override selected at launch. */
    effort?: AgentEffort;
    /** @deprecated Compatibility bridge for snapshots and command builders. */
    skipPermissions?: boolean;
    /**
     * Session ids that already existed when this fresh agent launched. Used to
     * keep it from adopting a pre-existing session during reconciliation —
     * it may only attach to a session file that appeared after launch. Cleared
     * once attached.
     */
    baselineSessionIds?: string[];
    /** Restored tabs stay dormant until the user explicitly resumes them. */
    launchState?: "live" | "dormant";
    /** Exempts a resumable live agent from automatic idle sleeping. */
    keepAlive?: boolean;
}

export interface PtyDirectCommand {
    program: string;
    args: string[];
    profile?: {
        configPath?: string;
        environmentKeys?: string[];
    };
}

export type AgentBackendState = "unknown" | "working" | "blocked" | "idle" | "stopped";
export type AgentPresentationState = AgentBackendState | "done";

export interface AgentRuntimeState {
    state: AgentPresentationState;
    backendState: AgentBackendState;
    unread: boolean;
    updatedAt: number;
    lastWorkedAt?: number;
    sequence: number;
    source: "screen" | "activity" | "process" | "fallback" | "acp";
    confidence: "high" | "medium" | "low";
    reason: string;
    matchedRule?: string;
}

/** Identity Sikemux attaches to every shell it owns. Runtime-only. */
export interface PtyContext {
    sessionId: string;
    sessionName: string;
    sessionKind: SessionKind;
    project?: string;
    windowId?: string;
    paneId?: string;
    agentId?: string;
    agentType?: AgentType;
    initialPromptSubmitted?: boolean;
    /** Opt-in local shell metadata; never an authorization signal. */
    shellIntegration?: boolean;
}

export type RailDensity = "comfortable" | "compact";

/** What the diff tab is reviewing: a changed file, or a whole commit. */
export type DiffTarget = { kind: "worktree"; path: string } | { kind: "commit"; rev: string; subject: string };

/** Which panel the workspace rail is showing. */

export interface Session {
    id: string;
    name: string;
    kind: SessionKind;
    cwd: string;
    pinned: boolean;
    activeWindowId: string;
}

/**
 * One entry in a session's tab strip: a window, narrowed to one of its
 * documents when the window holds documents.
 *
 * A window holding its own documents contributes one entry per document rather
 * than a single entry for itself, so they sit in the strip beside terminals and
 * agents instead of in a second tab bar inside the pane. The window's role says
 * what `doc` is — a file path for an editor, a request path for a Bruno
 * workspace — so the entry needs no kind of its own.
 */
export interface TabRef {
    id: string;
    doc?: string;
}

export interface RecentEntry {
    kind: SessionKind;
    name: string;
    cwd: string;
}

export interface ProjectRoot {
    path: string;
    depth: number;
    /**
     * Index the root itself as a project, git repo or not, on top of whatever
     * `depth` finds beneath it. Replaces the separate pinned-projects list.
     */
    selfIndex?: boolean;
}

export interface PinnedProject {
    path: string;
}

export interface Rect {
    x: number;
    y: number;
    w: number;
    h: number;
}

export interface Divider {
    splitId: string;
    index: number;
    dir: SplitDir;
    rect: Rect;
    at: number;
}

export type FocusDir = "left" | "right" | "up" | "down";

export type PickerMode = "all" | "projects" | "ssh";
