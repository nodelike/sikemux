import { create } from "zustand";
import { enableMapSet, produce, type Draft } from "immer";
import { DEFAULT_THEME_ID, type Theme } from "../themes";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../terminal/fontSize";
import { DEFAULT_CHAT_TEXT_SCALE } from "../chat/textScale";
import { DEFAULT_EDITOR_TEXT_SCALE } from "../editor/textScale";
import type { KeybindingOverrides } from "../keybindings";
import type { CustomCommand } from "../commands/registry";
import { DEFAULT_PROVIDER_PROFILES, DEFAULT_PROVIDER_PROFILE_SELECTION } from "./types";

enableMapSet();
import { makePane, newId } from "./layout";
import type { GitCmdEntry, GitModal } from "./gitTypes";
import type { BrowserSnapshot } from "../api/browser";
import type {
    Agent,
    AgentPermissionMode,
    AwsService,
    BrowserPaneView,
    EcsLevel,
    EditorPaneView,
    CliPendingEditorOpen,
    GitPaneView,
    BrunoView,
    GlobalSearchView,
    PickerMode,
    ProjectRoot,
    ProviderProfile,
    ProviderProfileSelection,
    RecentEntry,
    RundeckSettings,
    RailDensity,
    DiffTarget,
    RundeckView,
    Session,
    SessionSwitcherView,
    Window,
} from "./types";

export interface DomainState {
    sessions: Record<string, Session>;
    windows: Record<string, Window>;
    agents: Record<string, Agent>;

    sessionOrder: string[];
    windowsBySession: Record<string, string[]>;

    activeSessionId: string;

    recent: RecentEntry[];

    projectRoots: ProjectRoot[];
    /** Imported Bruno (API) workspace collection paths, most-recent-first. Survive session close so they stay reopenable. */
    brunoWorkspaces: string[];
    themeId: string;
    themeMode: "manual" | "system";
    systemLightThemeId: string;
    systemDarkThemeId: string;
    /** User-defined themes, derived from a built-in or another custom theme via the theme editor. */
    customThemes: Theme[];
    uiTextScale: number;
    terminalFontSize: number;
    chatTextScale: number;
    editorTextScale: number;
    windowOpacity: number;
    windowBlur: number;
    cloudBrowser: string;
    cloudBrowserShortcut: string;
    keybindingOverrides: KeybindingOverrides;
    awsProfile: string | null;
    awsService: AwsService;
    sideRailOpen: boolean;
    agentRailOpen: boolean;
    diffTarget: Record<string, DiffTarget | null>;
    zenMode: boolean;
    rundeck: RundeckSettings;
    restoreAgentTabs: boolean;
    railDensity: RailDensity;
    onboardingComplete: boolean;
    lastSeenVersion: string;
    customCommands: CustomCommand[];
    updateChannel: "stable" | "nightly";
    lastReleaseNotes: { version: string; notes: string | null; date: string | null } | null;
    recentCommandKeys: string[];
    /** Non-secret launch profiles and the per-agent defaults that reference them. */
    providerProfiles: ProviderProfile[];
    selectedProviderProfileIds: ProviderProfileSelection;
    defaultAgentPermissionMode: AgentPermissionMode;
}

export type UpdateOperationState = "available" | "preparing" | "downloading" | "installing" | "restarting" | "error";

export interface PendingUpdate {
    version: string;
    currentVersion: string;
    notes: string | null;
    date: string | null;
    state: UpdateOperationState;
    error: string | null;
    downloadedBytes: number;
    totalBytes: number | null;
}

/** Outcome of the most recent update check. A failed background check has no
 *  other surface, so About reads this instead of leaving the app silent. */
export interface UpdateCheckOutcome {
    at: number;
    channel: "stable" | "nightly";
    error: string | null;
}

export interface ViewState {
    home: string;

    pickerOpen: boolean;
    pickerMode: PickerMode;
    agentPaletteOpen: boolean;
    filePaletteOpen: boolean;
    newTabPaletteOpen: boolean;
    rundeckJobPaletteOpen: boolean;
    brunoReqPaletteOpen: boolean;
    brunoEnvPaletteOpen: boolean;
    settingsOpen: boolean;
    awsAuthModal: { profile: string; ssoStartUrl: string | null } | null;
    zoomedPaneId: string | null;
    sessionSwitcher: SessionSwitcherView | null;

    editorViews: Record<string, EditorPaneView>;
    /* Which agent a browser pane is showing, by pane id. The pane is an
       ordinary leaf in the layout; this is the only thing tying it back. */
    browserPanes: Record<string, string>;
    /** Each browsing agent's tab strip as the app last heard it, by agent id. */
    browserStrips: Record<string, BrowserSnapshot>;
    /** Tabs a restored pane is holding until someone looks at it, by pane id. */
    browserRestores: Record<string, BrowserPaneView>;
    /** Runtime-only file opens claimed from the CLI broker, keyed by editor pane. */
    pendingEditorOpens: Record<string, CliPendingEditorOpen[]>;
    dirtyEditorPaths: Record<string, string[]>;
    gitViews: Record<string, GitPaneView>;
    ecsViews: Record<string, EcsLevel>;
    rundeckViews: Record<string, RundeckView>;
    brunoViews: Record<string, BrunoView>;
    expandedBillingMonth: Record<string, string | null>;

    gitModal: GitModal | null;
    gitCmdLog: GitCmdEntry[];
    gitCmdLogOpen: boolean;

    globalSearchBySession: Record<string, GlobalSearchView>;

    /** Runtime-only PTY activity for live agents. Never persisted or hydrated. */
    agentActivity: Record<string, import("./types").AgentRuntimeState>;
    /** How many background shells, monitors and subagents each agent still has going. */
    agentBackgroundWork: Record<string, number>;

    commandPaletteOpen: boolean;
    onboardingOpen: boolean;
    diagnosticsOpen: boolean;
    whatsNewOpen: boolean;
    commandPopup: { id: string; title: string; startup: string; cwd: string; context: import("./types").PtyContext } | null;
    terminalTitles: Record<string, string>;
    lastSessionId: string | null;

    pendingUpdate: PendingUpdate | null;
    lastUpdateCheck: UpdateCheckOutcome | null;
}

export type StoreState = DomainState & ViewState;

function initialSession(): {
    session: Session;
    window: Window;
} {
    const sessId = newId("sess");
    const pane = makePane("", { kind: "terminal" });
    const win: Window = {
        id: newId("win"),
        name: "1",
        role: "term",
        root: pane,
        activePaneId: pane.id,
    };
    const session: Session = {
        id: sessId,
        name: "main",
        kind: "command",
        cwd: "",
        deploy: null,
        pinned: false,
        activeWindowId: win.id,
    };
    return { session, window: win };
}

export const useStore = create<StoreState>(() => {
    const { session, window } = initialSession();
    return {
        sessions: { [session.id]: session },
        windows: { [window.id]: window },
        agents: {},
        sessionOrder: [session.id],
        windowsBySession: { [session.id]: [window.id] },
        activeSessionId: session.id,
        recent: [],
        projectRoots: [],
        brunoWorkspaces: [],
        themeId: DEFAULT_THEME_ID,
        themeMode: "manual",
        systemLightThemeId: "aura-day",
        systemDarkThemeId: DEFAULT_THEME_ID,
        customThemes: [],
        uiTextScale: 1,
        terminalFontSize: DEFAULT_TERMINAL_FONT_SIZE,
        chatTextScale: DEFAULT_CHAT_TEXT_SCALE,
        editorTextScale: DEFAULT_EDITOR_TEXT_SCALE,
        windowOpacity: 1,
        windowBlur: 0,
        cloudBrowser: "",
        cloudBrowserShortcut: "",
        keybindingOverrides: {},
        awsProfile: null,
        awsService: "ecs",
        sideRailOpen: true,
        agentRailOpen: true,
        diffTarget: {},
        zenMode: false,
        rundeck: {
            activeProject: "",
            activeEnvFolder: null,
            prodEnvs: ["prod", "production"],
        },
        restoreAgentTabs: true,
        railDensity: "comfortable",
        onboardingComplete: false,
        lastSeenVersion: "",
        customCommands: [],
        updateChannel: "stable",
        lastReleaseNotes: null,
        recentCommandKeys: [],
        providerProfiles: DEFAULT_PROVIDER_PROFILES.map((profile) => ({ ...profile })),
        selectedProviderProfileIds: { ...DEFAULT_PROVIDER_PROFILE_SELECTION },
        defaultAgentPermissionMode: "bypass",

        home: "",
        pickerOpen: false,
        pickerMode: "all",
        agentPaletteOpen: false,
        filePaletteOpen: false,
        newTabPaletteOpen: false,
        rundeckJobPaletteOpen: false,
        brunoReqPaletteOpen: false,
        brunoEnvPaletteOpen: false,
        settingsOpen: false,
        awsAuthModal: null,
        zoomedPaneId: null,
        sessionSwitcher: null,
        editorViews: {},
        browserPanes: {},
        browserStrips: {},
        browserRestores: {},
        pendingEditorOpens: {},
        dirtyEditorPaths: {},
        gitViews: {},
        ecsViews: {},
        rundeckViews: {},
        brunoViews: {},
        expandedBillingMonth: {},
        gitModal: null,
        gitCmdLog: [],
        gitCmdLogOpen: false,
        globalSearchBySession: {},
        agentActivity: {},
        agentBackgroundWork: {},
        commandPaletteOpen: false,
        onboardingOpen: false,
        diagnosticsOpen: false,
        whatsNewOpen: false,
        commandPopup: null,
        terminalTitles: {},
        lastSessionId: null,
        pendingUpdate: null,
        lastUpdateCheck: null,
    };
});

export const getState = useStore.getState;
export const setState = useStore.setState;

export function mutate(fn: (draft: Draft<StoreState>) => void): void {
    setState((st) => produce(st, fn));
}
