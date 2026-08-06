import type { Theme } from "../../themes";
import type { CustomCommand } from "../../commands/registry";
import type { KeybindingOverrides } from "../../keybindings";
import type {
    Agent,
    AwsService,
    NotificationPreferences,
    PinnedProject,
    ProjectRoot,
    RailDensity,
    RecentEntry,
    RundeckSettings,
    Session,
    Window,
} from "./domain";
import type { EditorPaneView } from "./view";

export type PersistedSession = Omit<Session, "bruno"> & {
    bruno?: Pick<NonNullable<Session["bruno"]>, "collectionPath" | "selectedEnvs"> | null;
};

/** Safe restart record. Startup commands and runtime evidence are never serialized. */
export type PersistedAgent = Pick<Agent, "id" | "type" | "title" | "resumeId" | "skipPermissions">;

export interface PersistedSnapshot {
    version: number;
    sessions: PersistedSession[];
    windowsBySession: Record<string, Window[]>;
    agentsBySession: Record<string, PersistedAgent[]>;
    sessionOrder: string[];
    activeSessionId: string;
    recent: RecentEntry[];
    prefs: PersistedPrefs;
    editorViews: Record<string, EditorPaneView>;
}

export interface PersistedPrefs {
    pinnedProjects: PinnedProject[];
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
    leftRailOpen: boolean;
    rightRailOpen: boolean;
    zenMode: boolean;
    rundeck?: RundeckSettings;
    restoreAgentTabs?: boolean;
    autoResumeAgents?: boolean;
    notificationPreferences?: NotificationPreferences;
    railDensity?: RailDensity;
    onboardingComplete?: boolean;
    lastSeenVersion?: string;
    customCommands?: CustomCommand[];
    updateChannel?: "stable" | "preview";
    lastReleaseNotes?: { version: string; notes: string | null; date: string | null } | null;
    recentCommandKeys?: string[];
}
