import { ACCOUNT_AGENT_TYPES, agentBrand, agentLabel, type AccountAgentType, type AgentType } from "../../agents/catalog";
import type { PluginKind } from "../../plugins/kinds";
import type { SpaceIcon } from "../projectSpaces";

/**
 * How a split arranges its children.
 *
 * `row` and `column` tile them side by side. `stack` puts them in the same
 * place and shows one at a time, which is what a tab strip is — the children
 * are tabs, and the active pane decides which is on top.
 */
export type SplitDir = "row" | "column" | "stack";
import type { CorePaneKind, WindowRole } from "../windowRoles";

export type { CorePaneKind, WindowRole };
export type PaneKind = CorePaneKind | PluginKind;

export interface PaneNode {
    type: "pane";
    id: string;
    cwd: string;
    kind: PaneKind;
    title: string;
    startup?: string;
    /** The tab this pane had before it was split into another, which it takes back when it moves out. */
    tab?: { name: string; role: WindowRole };
    /** The terminal core session this pane shows, which outlives the app. */
    ptyId?: number;
    /** The host the terminal runs on, by its core's key, when that is not this one. */
    host?: string;
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

export interface Window {
    id: string;
    name: string;
    role: WindowRole;
    root: LayoutNode;
    activePaneId: string;
    fixed?: boolean;
    /** The name the person gave this tab, shown instead of the title its terminal reports. */
    customName?: string;
    /** Runtime-only windows are deliberately omitted from persistence. */
    transient?: true;
}

export type { AgentType };

/** Whether an agent asks before it acts (the provider's own approvals and sandbox) or acts without asking. */
export type AgentPermissionMode = "workspace-write" | "bypass";

/** Provider-neutral reasoning levels; unsupported levels are normalized per CLI. */
export const AGENT_EFFORTS = ["off", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"] as const;
export type AgentEffort = (typeof AGENT_EFFORTS)[number];

/**
 * Durable, non-secret configuration for an agent executable.
 * `environmentKeys` stores names only; credential values remain in the user's
 * shell, provider config, or OS credential store.
 */
export interface ProviderProfile {
    id: string;
    name: string;
    provider: AccountAgentType;
    accent: string;
    executablePath?: string;
    configPath?: string;
    environmentKeys?: string[];
}

export type ProviderProfileSelection = Partial<Record<AgentType, string>>;

const builtinProfileId = (type: AccountAgentType): string => `builtin-${type}`;

export const DEFAULT_PROVIDER_PROFILES: readonly ProviderProfile[] = ACCOUNT_AGENT_TYPES.map((type) => ({
    id: builtinProfileId(type),
    name: agentLabel(type),
    provider: type,
    accent: agentBrand(type),
}));

export const DEFAULT_PROVIDER_PROFILE_SELECTION: Readonly<ProviderProfileSelection> = Object.fromEntries(
    ACCOUNT_AGENT_TYPES.map((type) => [type, builtinProfileId(type)]),
);

export interface Agent {
    id: string;
    type: AgentType;
    /** What the core calls this agent, such as Otter: the word the person and other agents address it by. */
    name?: string;
    /** The agent that started this one, by id, when an agent did rather than the person. */
    lead?: string;
    title: string;
    resumeId?: string;
    createdAt?: number;
    permissionMode?: AgentPermissionMode;
    /** Non-secret provider profile selected for this launch. */
    profileId?: string;
    /** Health-checked executable used for this launch. */
    executablePath?: string;
    /** Effective launch directory. */
    cwd?: string;
    /** Optional provider model override selected at launch. */
    model?: string;
    /** The model provider a chat runs on, by catalogue id, in place of its maker's account. */
    modelProvider?: string;
    /** Optional provider reasoning-effort override selected at launch. */
    effort?: AgentEffort;
    /**
     * Session ids that already existed when this fresh agent launched. Used to
     * keep it from adopting a pre-existing session during reconciliation —
     * it may only attach to a session file that appeared after launch. Cleared
     * once attached.
     */
    /** Restored tabs stay dormant until the user explicitly resumes them. */
    launchState?: "live" | "dormant";
    /** Exempts a resumable live agent from automatic idle sleeping. */
    keepAlive?: boolean;
    /** The user named this agent, so titles from the provider no longer replace it. */
    renamed?: boolean;
    /** The git worktree this chat was moved into before its first message. */
    worktree?: AgentWorktree;
    /** The terminal core session running this agent's TUI, which outlives the app. */
    ptyId?: number;
    /** The host the chat runs on, by its core's key, when that is not this one. */
    host?: string;
    /** The character drawn for this agent. Without a pack and shape it follows the default style in settings. */
    buddy?: AgentBuddy;
}

export interface AgentBuddy {
    /** A slot in the pack's palette, given out so open agents rarely share one. */
    color: number;
    pack?: string;
    shape?: string;
}

export interface AgentWorktree {
    /** The main checkout, which git commands about the worktree run against. */
    repo: string;
    path: string;
    branch: string;
    /** The branch the project was on when the worktree was cut, if it was on one. */
    base: string | null;
    startSha: string;
}

export interface PtyDirectCommand {
    program: string;
    args: string[];
    profile?: {
        configPath?: string;
        environmentKeys?: string[];
    };
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
/** What a message sent while the agent works does: goes into the running turn, or waits for it to end. */
export type MidTurnSend = "steer" | "queue";
/** Whether the tab rail lists the open project or every open project. */
export type TabRailScope = "project" | "all";
/** Which of its pages the tab rail shows: the project's agents, its terminals or its open files. */
export type TabRailPage = "agents" | "terminals" | "files";

/** A saved chat kept at the top of the tab rail, open or closed. */
export interface PinnedChat {
    agent: AgentType;
    /** The agent's own id for the conversation, which resumes it. */
    id: string;
    title: string;
    /** The project folder the chat ran in. */
    project: string;
}

/** What the diff tab is reviewing: a changed file, or a whole commit. */
export type DiffTarget = { kind: "worktree"; path: string } | { kind: "commit"; rev: string; subject: string };

/** Which panel the workspace rail is showing. */

/** A set of projects the person makes to see on their own, so other work stays out of sight without being closed. */
export interface ProjectSpace {
    id: string;
    name: string;
    /** Empty when the person removed it; the space then shows only its name. */
    icon: SpaceIcon | "";
}

export interface Session {
    id: string;
    name: string;
    kind: SessionKind;
    cwd: string;
    pinned: boolean;
    activeWindowId: string;
    /** The host whose folder this is, by its core's key, for a project only another host holds. */
    host?: string;
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
