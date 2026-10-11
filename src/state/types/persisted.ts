import type { Theme } from "../../themes";
import type { CustomCommand } from "../../commands/registry";
import type { KeybindingOverrides } from "../../commands/keybindings";
import type { HeldRelease } from "../../api/releases";
import type {
    Agent,
    AgentPermissionMode,
    AgentType,
    ProjectRoot,
    ProjectSpace,
    ProviderProfile,
    ProviderProfileSelection,
    RailDensity,
    AgentRailScope,
    RecentEntry,
    Session,
    Window,
} from "./domain";
import type { EditorPaneView } from "./view";
import type { PersistedWorkbenchItemEnvelope } from "../../workbench/registry";

export type PersistedSession = Session;

/** Safe restart record. Startup commands and runtime evidence are never serialized. */
export type PersistedAgent = Pick<
    Agent,
    | "id"
    | "type"
    | "title"
    | "resumeId"
    | "permissionMode"
    | "profileId"
    | "executablePath"
    | "cwd"
    | "model"
    | "effort"
    | "skipPermissions"
    | "keepAlive"
    | "renamed"
    | "worktree"
    | "ptyId"
>;

export interface PersistedSnapshot {
    version: number;
    sessions: PersistedSession[];
    windowsBySession: Record<string, Window[]>;
    /** Launch records for the agent windows above; the window says which session owns it. */
    agents: PersistedAgent[];
    /** v3-v7 input; agents lived beside their session rather than in a window. */
    agentsBySession?: Record<string, PersistedAgent[]>;
    sessionOrder: string[];
    activeSessionId: string;
    recent: RecentEntry[];
    prefs: PersistedPrefs;
    /** Versioned item-owned state. Live processes, buffers, and credentials are excluded. */
    itemStates: Record<string, PersistedWorkbenchItemEnvelope>;
    /** v3-v6 compatibility input; v7 writers only emit itemStates. */
    editorViews?: Record<string, EditorPaneView>;
}

export interface PersistedPrefs {
    projectRoots: ProjectRoot[];
    themeId: string;
    customThemes?: Theme[];
    uiTextScale?: number;
    paneShader?: boolean;
    paneImage?: string | null;
    terminalFontSize?: number;
    chatTextScale?: number;
    editorTextScale?: number;
    windowOpacity: number;
    windowBlur: number;
    cloudBrowser: string;
    cloudBrowserShortcut: string;
    keybindingOverrides?: KeybindingOverrides;
    sideRailOpen: boolean;
    agentRailOpen: boolean;
    sideRailWidth?: number;
    agentRailWidth?: number;
    pluginSettings?: Record<string, unknown>;
    disabledPlugins?: string[];
    restoreAgentTabs?: boolean;
    spaces?: ProjectSpace[];
    projectSpaces?: Record<string, string>;
    activeSpaceId?: string | null;
    agentNotifications?: boolean;
    notch?: unknown;
    voiceDictation?: boolean;
    iosSimulator?: boolean;
    simulatorFrame?: boolean;
    notificationsIntroduced?: boolean;
    keptRunningNoticeShown?: boolean;
    railDensity?: RailDensity;
    agentRailAllAgents?: boolean;
    accountAutoSwitch?: Partial<Record<"claude" | "codex", boolean>>;
    agentRailScope?: AgentRailScope;
    onboardingComplete?: boolean;
    lastSeenVersion?: string;
    customCommands?: CustomCommand[];
    updateChannel?: "stable" | "nightly";
    shareUsageData?: boolean;
    lastReleaseNotes?: HeldRelease | null;
    recentCommandKeys?: string[];
    /** Non-secret provider launch profiles. Credential values are never part of this shape. */
    providerProfiles?: ProviderProfile[];
    selectedProviderProfileIds?: ProviderProfileSelection;
    defaultAgentPermissionMode?: AgentPermissionMode;
    lastAgentType?: AgentType | null;
    languageServerTrust?: Record<string, boolean>;
    agentWorktreeDefaults?: Record<string, boolean>;
}
