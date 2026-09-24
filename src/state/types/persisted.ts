import type { Theme } from "../../themes";
import type { CustomCommand } from "../../commands/registry";
import type { KeybindingOverrides } from "../../keybindings";
import type {
    Agent,
    AgentPermissionMode,
    ProjectRoot,
    ProviderProfile,
    ProviderProfileSelection,
    RailDensity,
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
    zenMode: boolean;
    pluginSettings?: Record<string, unknown>;
    disabledPlugins?: string[];
    restoreAgentTabs?: boolean;
    autoResumeAgents?: boolean;
    railDensity?: RailDensity;
    onboardingComplete?: boolean;
    lastSeenVersion?: string;
    customCommands?: CustomCommand[];
    updateChannel?: "stable" | "nightly";
    lastReleaseNotes?: { version: string; notes: string | null; date: string | null } | null;
    recentCommandKeys?: string[];
    /** Non-secret provider launch profiles. Credential values are never part of this shape. */
    providerProfiles?: ProviderProfile[];
    selectedProviderProfileIds?: ProviderProfileSelection;
    defaultAgentPermissionMode?: AgentPermissionMode;
}
