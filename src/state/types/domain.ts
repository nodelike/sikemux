export type SplitDir = "row" | "column";
export type PaneKind = "terminal" | "editor" | "git" | "diff" | "aws" | "search" | "rundeck" | "bruno";

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

export type SessionKind = "project" | "command" | "ssh" | "aws" | "rundeck" | "bruno";

export type WindowRole = "term" | "files" | "git" | "diff" | "search" | "aws" | "rundeck" | "bruno" | "ssh-config" | "named";

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
    { id: "builtin-codex", name: "Codex", provider: "codex", accent: "#10a37f" },
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
export type RailTab = "agents" | "files" | "changes" | "search";

/** A resolved Rundeck deploy location for a service: a project plus an env subfolder. */
export interface DeployRef {
    project: string;
    folder: string | null;
}

/**
 * Durable per-session state for a Bruno (API) workspace. Lives on the Session so
 * it persists with the existing `sessions` slice — no persist version bump.
 * Secret var values are entered in-app (not stored in .bru files); `drafts` holds
 * edited-but-unsaved request text keyed by file path.
 */
export interface BrunoSessionState {
    collectionPath: string;
    /** selected environment id per collection root (workspaces hold many collections) */
    selectedEnvs: Record<string, string>;
    secretVars: Record<string, string>;
    drafts: Record<string, string>;
}

export interface Session {
    id: string;
    name: string;
    kind: SessionKind;
    cwd: string;
    /** Selected Rundeck deploy location for this session's service, when picked. */
    deploy?: DeployRef | null;
    /** Bruno (API) workspace state — present only when kind === "bruno". */
    bruno?: BrunoSessionState | null;
    pinned: boolean;
    activeWindowId: string;
    activeAgentId: string | null;
    view: "windows" | "agent";
}

/**
 * One entry in a session's tab strip.
 *
 * An editor contributes one `file` entry per open document rather than a single
 * entry for itself, so its documents sit in the strip beside terminals and
 * agents instead of in a second tab bar inside the pane. `id` is the window in
 * every case; a file entry names the document it selects within it.
 */
export type WorkspaceTabRef = { kind: "window"; id: string } | { kind: "agent"; id: string } | { kind: "file"; id: string; path: string };

export interface RecentEntry {
    kind: SessionKind;
    name: string;
    cwd: string;
}

export type AwsService = "ecs" | "ec2" | "lambda" | "sqs" | "billing" | "s3";
export const AWS_SERVICES: AwsService[] = ["ecs", "ec2", "lambda", "sqs", "billing", "s3"];

export interface RundeckSettings {
    activeProject: string;
    activeEnvFolder: string | null;
    prodEnvs: string[];
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

export type PickerMode = "all" | "projects" | "ssh" | "bruno";
