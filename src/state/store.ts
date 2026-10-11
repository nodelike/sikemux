import { DEFAULT_NOTCH_SETTINGS } from "../notch/notchSettings";
import type { PluginManifest } from "../api/plugins";
import { create } from "zustand";
import { enableMapSet, produce, type Draft } from "immer";
import { DEFAULT_THEME_ID, type Theme } from "../themes";
import { DEFAULT_TERMINAL_FONT_SIZE } from "../terminal/fontSize";
import { DEFAULT_CHAT_TEXT_SCALE } from "../chat/textScale";
import { DEFAULT_EDITOR_TEXT_SCALE } from "../editor/textScale";
import type { KeybindingOverrides } from "../commands/keybindings";
import type { CustomCommand } from "../commands/registry";
import type { SettingsPageId } from "../settings/settingsIndex";
import { RAIL_WIDTH } from "../lib/railWidths";
import { DEFAULT_PROVIDER_PROFILES, DEFAULT_PROVIDER_PROFILE_SELECTION } from "./types";

enableMapSet();
import { makePane, newId } from "./layout";
import type { GitCmdEntry, GitModal } from "./gitTypes";
import type { BrowserSnapshot } from "../api/browser";
import type { HeldRelease, ReleaseCredits } from "../api/releases";
import type {
    Agent,
    AgentPermissionMode,
    AgentType,
    Desk,
    DeskView,
    EditorPaneView,
    CliPendingEditorOpen,
    GitPaneView,
    GlobalSearchView,
    PickerMode,
    ProjectRoot,
    ProjectSpace,
    ProviderProfile,
    ProviderProfileSelection,
    RecentEntry,
    RailDensity,
    AgentRailScope,
    DiffTarget,
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
    themeId: string;
    /** User-defined themes, derived from a built-in or another custom theme via the theme editor. */
    customThemes: Theme[];
    uiTextScale: number;
    paneShader: boolean;
    /** A picture on disk that panes show, dithered, in place of the grain. */
    paneImage: string | null;
    terminalFontSize: number;
    chatTextScale: number;
    editorTextScale: number;
    windowOpacity: number;
    windowBlur: number;
    cloudBrowser: string;
    cloudBrowserShortcut: string;
    keybindingOverrides: KeybindingOverrides;
    sideRailOpen: boolean;
    agentRailOpen: boolean;
    sideRailWidth: number;
    agentRailWidth: number;
    diffTarget: Record<string, DiffTarget | null>;
    /** Each plugin's own settings, by plugin id, in whatever shape the plugin decodes. */
    pluginSettings: Readonly<Record<string, unknown>>;
    /** Plugins switched off in Settings; they are built in but act as if absent. */
    disabledPlugins: readonly string[];
    restoreAgentTabs: boolean;
    spaces: readonly ProjectSpace[];
    /** The space id each project belongs to, by project folder, so it outlives closing the project. */
    projectSpaces: Readonly<Record<string, string>>;
    /** The space the rail shows, or null for every project. */
    activeSpaceId: string | null;
    agentNotifications: boolean;
    notch: import("../notch/notchSettings").NotchSettings;
    voiceDictation: boolean;
    /** Agents get the iOS Simulator tools on a Mac that can run it. */
    iosSimulator: boolean;
    /** The simulator tab draws the device's frame and side buttons around its screen. */
    simulatorFrame: boolean;
    notificationsIntroduced: boolean;
    /** The person was told once that terminals keep running after Sikemux quits. */
    keptRunningNoticeShown: boolean;
    railDensity: RailDensity;
    /** The agent rail shows every CLI's chats instead of one provider's. */
    agentRailAllAgents: boolean;
    /** Providers whose chats move to the next signed-in account when one runs out of usage. */
    accountAutoSwitch: Partial<Record<"claude" | "codex", boolean>>;
    agentRailScope: AgentRailScope;
    onboardingComplete: boolean;
    lastSeenVersion: string;
    customCommands: CustomCommand[];
    updateChannel: "stable" | "nightly";
    shareUsageData: boolean;
    lastReleaseNotes: HeldRelease | null;
    recentCommandKeys: string[];
    /** Non-secret launch profiles and the per-agent defaults that reference them. */
    providerProfiles: ProviderProfile[];
    selectedProviderProfileIds: ProviderProfileSelection;
    defaultAgentPermissionMode: AgentPermissionMode;
    /** The agent ⌘N starts: whichever was launched last. */
    lastAgentType: AgentType | null;
    /** Whether each project, by root path, may start its language servers. A project absent here has not been asked. */
    languageServerTrust: Record<string, boolean>;
    /** Whether a new chat in each project, by root path, starts with its Worktree switch on. */
    agentWorktreeDefaults: Record<string, boolean>;
}

export type UpdateOperationState = "available" | "preparing" | "downloading" | "installing" | "restarting" | "error";

export interface PendingUpdate {
    version: string;
    currentVersion: string;
    notes: string | null;
    date: string | null;
    credits: ReleaseCredits | null;
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
    /** Plugins compiled into this build, as the native host reports them. */
    pluginManifests: readonly PluginManifest[];

    pickerOpen: boolean;
    pickerMode: PickerMode;
    agentPaletteOpen: boolean;
    filePaletteOpen: boolean;
    newTabPaletteOpen: boolean;
    /** The agent whose desk has its address open in the middle of the page, from ⌘L. */
    deskAddressOpen: string | null;
    settingsOpen: boolean;
    settingsPage: SettingsPageId;
    /** A section or row title the settings pane scrolls to once it opens. */
    settingsTarget: string | null;
    zoomedPaneId: string | null;
    sessionSwitcher: SessionSwitcherView | null;

    editorViews: Record<string, EditorPaneView>;
    /* Which agent a desk pane is showing, by pane id. The pane is an
       ordinary leaf in the layout; this is the only thing tying it back. */
    deskPanes: Record<string, string>;
    /** Each agent's desk, by agent id. It outlives the pane, so hiding a desk keeps what is on it. */
    desks: Record<string, Desk>;
    /** Each browsing agent's tab strip as the app last heard it, by agent id. */
    browserStrips: Record<string, BrowserSnapshot>;
    /** Pages a restored desk is holding until someone looks at it, by pane id. */
    deskRestores: Record<string, DeskView>;
    /** Runtime-only file opens claimed from the CLI broker, keyed by editor pane. */
    pendingEditorOpens: Record<string, CliPendingEditorOpen[]>;
    dirtyEditorPaths: Record<string, string[]>;
    gitViews: Record<string, GitPaneView>;

    gitModal: GitModal | null;
    gitCmdLog: GitCmdEntry[];
    gitCmdLogOpen: boolean;

    globalSearchBySession: Record<string, GlobalSearchView>;

    /** Runtime-only PTY activity for live agents. Never persisted or hydrated. */
    agentActivity: Record<string, import("./types").AgentRuntimeState>;
    /** How many background shells, monitors and subagents each agent still has going. */
    agentBackgroundWork: Record<string, number>;
    /** How many of those are subagents, counted on their own so a tab can show them. */
    agentSubagents: Record<string, number>;

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
        name: "Terminal",
        role: "term",
        root: pane,
        activePaneId: pane.id,
    };
    const session: Session = {
        id: sessId,
        name: "main",
        kind: "command",
        cwd: "",
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
        themeId: DEFAULT_THEME_ID,
        customThemes: [],
        uiTextScale: 1,
        paneShader: true,
        paneImage: null,
        terminalFontSize: DEFAULT_TERMINAL_FONT_SIZE,
        chatTextScale: DEFAULT_CHAT_TEXT_SCALE,
        editorTextScale: DEFAULT_EDITOR_TEXT_SCALE,
        windowOpacity: 1,
        windowBlur: 0,
        cloudBrowser: "",
        cloudBrowserShortcut: "",
        keybindingOverrides: {},
        sideRailOpen: true,
        agentRailOpen: true,
        sideRailWidth: RAIL_WIDTH.start.initial,
        agentRailWidth: RAIL_WIDTH.end.initial,
        diffTarget: {},
        pluginSettings: {},
        disabledPlugins: [],
        restoreAgentTabs: true,
        spaces: [],
        projectSpaces: {},
        activeSpaceId: null,
        agentNotifications: true,
        notch: DEFAULT_NOTCH_SETTINGS,
        voiceDictation: false,
        iosSimulator: true,
        simulatorFrame: true,
        notificationsIntroduced: false,
        keptRunningNoticeShown: false,
        railDensity: "comfortable",
        agentRailAllAgents: false,
        accountAutoSwitch: {},
        agentRailScope: "project",
        onboardingComplete: false,
        lastSeenVersion: "",
        customCommands: [],
        updateChannel: "stable",
        shareUsageData: true,
        lastReleaseNotes: null,
        recentCommandKeys: [],
        providerProfiles: DEFAULT_PROVIDER_PROFILES.map((profile) => ({ ...profile })),
        selectedProviderProfileIds: { ...DEFAULT_PROVIDER_PROFILE_SELECTION },
        defaultAgentPermissionMode: "bypass",
        lastAgentType: null,
        languageServerTrust: {},
        agentWorktreeDefaults: {},

        home: "",
        pluginManifests: [],
        pickerOpen: false,
        pickerMode: "all",
        agentPaletteOpen: false,
        filePaletteOpen: false,
        newTabPaletteOpen: false,
        deskAddressOpen: null,
        settingsOpen: false,
        settingsPage: "general",
        settingsTarget: null,
        zoomedPaneId: null,
        sessionSwitcher: null,
        editorViews: {},
        deskPanes: {},
        desks: {},
        browserStrips: {},
        deskRestores: {},
        pendingEditorOpens: {},
        dirtyEditorPaths: {},
        gitViews: {},
        gitModal: null,
        gitCmdLog: [],
        gitCmdLogOpen: false,
        globalSearchBySession: {},
        agentActivity: {},
        agentBackgroundWork: {},
        agentSubagents: {},
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
