import type { Theme } from "../../themes";
import type { CustomCommand } from "../../commands/registry";
import type { KeybindingOverrides } from "../../commands/keybindings";
import type { HeldRelease } from "../../api/releases";
import type {
    Agent,
    AgentType,
    ProjectRoot,
    ProviderProfile,
    ProviderProfileSelection,
    RailDensity,
    MidTurnSend,
    TabRailScope,
    TabRailPage,
    RecentEntry,
    Session,
    Window,
} from "./domain";
import type { EditorPaneView } from "./view";
import type { PersistedWorkbenchItemEnvelope } from "../../workbench/registry";

/** A project's name and folder are the core's, so a window keeps only which of its tabs is in front. */
export type PersistedSession = Session | Pick<Session, "id" | "kind" | "activeWindowId">;

/** Safe restart record. Startup commands and runtime evidence are never serialized. */
export type PersistedAgent = Pick<
    Agent,
    | "id"
    | "type"
    | "name"
    | "lead"
    | "title"
    | "resumeId"
    | "permissionMode"
    | "profileId"
    | "executablePath"
    | "cwd"
    | "model"
    | "modelProvider"
    | "effort"
    | "skipPermissions"
    | "keepAlive"
    | "renamed"
    | "worktree"
    | "ptyId"
    | "buddy"
>;

export interface PersistedSnapshot {
    version: number;
    sessions: PersistedSession[];
    windowsBySession: Record<string, Window[]>;
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
    buddies?: boolean;
    buddyPack?: string;
    buddyShape?: string;
    buddyMotion?: boolean;
    paneImage?: string | null;
    paneStrength?: number;
    terminalFontSize?: number;
    chatTextScale?: number;
    editorTextScale?: number;
    windowOpacity: number;
    windowBlur: number;
    cloudBrowser: string;
    cloudBrowserShortcut: string;
    keybindingOverrides?: KeybindingOverrides;
    sideRailOpen: boolean;
    tabRailOpen: boolean;
    sideRailWidth?: number;
    tabRailWidth?: number;
    pluginSettings?: Record<string, unknown>;
    restoreAgentTabs?: boolean;
    activeSpaceId?: string | null;
    agentNotifications?: boolean;
    notch?: unknown;
    voiceDictation?: boolean;
    iosSimulator?: boolean;
    simulatorFrame?: boolean;
    notificationsIntroduced?: boolean;
    keptRunningNoticeShown?: boolean;
    signInCardDismissed?: boolean;
    railDensity?: RailDensity;
    midTurnSend?: MidTurnSend;
    tabRailAllAgents?: boolean;
    tabRailScope?: TabRailScope;
    tabRailPage?: TabRailPage;
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
    lastAgentType?: AgentType | null;
    languageServerTrust?: Record<string, boolean>;
    agentWorktreeDefaults?: Record<string, boolean>;
    defaultGitRepos?: Record<string, string>;
}
