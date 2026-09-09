import type { Theme } from "../../themes";
import type { CustomCommand } from "../../commands/registry";
import type { KeybindingOverrides } from "../../keybindings";
import type {
    Agent,
    AgentPermissionMode,
    AwsService,
    ProjectRoot,
    ProviderProfile,
    ProviderProfileSelection,
    RailDensity,
    RailTab,
    RecentEntry,
    RundeckSettings,
    Session,
    Window,
} from "./domain";
import type { EditorPaneView } from "./view";
import type { PersistedWorkbenchItemEnvelope } from "../../workbench/registry";

export type PersistedSession = Omit<Session, "bruno"> & {
    bruno?: Pick<NonNullable<Session["bruno"]>, "collectionPath" | "selectedEnvs"> | null;
};

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
    agentsBySession: Record<string, PersistedAgent[]>;
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
    brunoWorkspaces?: string[];
    themeId: string;
    themeMode?: "manual" | "system";
    systemLightThemeId?: string;
    systemDarkThemeId?: string;
    customThemes?: Theme[];
    windowOpacity: number;
    windowBlur: number;
    cloudBrowser: string;
    cloudBrowserShortcut: string;
    keybindingOverrides?: KeybindingOverrides;
    awsProfile: string | null;
    awsService: AwsService;
    sideRailOpen: boolean;
    workspaceRailOpen: boolean;
    railTab?: RailTab;
    railChangesSplit?: number;
    zenMode: boolean;
    rundeck?: RundeckSettings;
    restoreAgentTabs?: boolean;
    autoResumeAgents?: boolean;
    railDensity?: RailDensity;
    onboardingComplete?: boolean;
    lastSeenVersion?: string;
    customCommands?: CustomCommand[];
    updateChannel?: "stable" | "preview";
    lastReleaseNotes?: { version: string; notes: string | null; date: string | null } | null;
    recentCommandKeys?: string[];
    /** Non-secret provider launch profiles. Credential values are never part of this shape. */
    providerProfiles?: ProviderProfile[];
    selectedProviderProfileIds?: ProviderProfileSelection;
    defaultAgentPermissionMode?: AgentPermissionMode;
}
