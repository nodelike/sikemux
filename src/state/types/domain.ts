export type SplitDir = "row" | "column";
export type PaneKind = "terminal" | "editor" | "git" | "aws" | "search" | "rundeck" | "bruno";

export interface PaneNode {
    type: "pane";
    id: string;
    cwd: string;
    kind: PaneKind;
    title: string;
    startup?: string;
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

export type WindowRole = "term" | "files" | "git" | "search" | "aws" | "rundeck" | "bruno" | "ssh-config" | "named";

export interface Window {
    id: string;
    name: string;
    role: WindowRole;
    root: LayoutNode;
    activePaneId: string;
    fixed?: boolean;
}

export type AgentType = "claude" | "codex" | "hermes" | "pi" | "opencode";

export interface Agent {
    id: string;
    type: AgentType;
    title: string;
    startup: string;
    resumeId?: string;
    createdAt?: number;
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
}

export type AgentBackendState = "unknown" | "working" | "blocked" | "idle";
export type AgentPresentationState = AgentBackendState | "done";

export interface AgentRuntimeState {
    state: AgentPresentationState;
    backendState: AgentBackendState;
    unread: boolean;
    updatedAt: number;
    sequence: number;
    source: "screen" | "activity" | "process" | "fallback";
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
}

export interface NotificationPreferences {
    enabled: boolean;
    onlyWhenUnfocused: boolean;
    sounds: boolean;
    soundStyle: "soft" | "bright";
    delayMs: number;
    quietHoursEnabled: boolean;
    quietHoursStart: string;
    quietHoursEnd: string;
    mutedAgentTypes: AgentType[];
}

export type RailDensity = "comfortable" | "compact";

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
