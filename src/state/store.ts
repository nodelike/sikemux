import { DEFAULT_NOTCH_SETTINGS } from "../notch/notchSettings";
import type { CodeHostChoice } from "../api/generated/wire";
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
import { DEFAULT_BUDDY_STYLE } from "../buddies/style";
import { DEFAULT_PROVIDER_PROFILES, DEFAULT_PROVIDER_PROFILE_SELECTION } from "./types";
import type { AccountAgentType } from "../agents/catalog";

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
    MidTurnSend,
    TabRailScope,
    TabRailPage,
    PinnedChat,
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
    /** Agents show a character in place of their harness logo. */
    buddies: boolean;
    /** The default character style: a pack id or "all", and a shape id or "mixed". */
    buddyPack: string;
    buddyShape: string;
    buddyMotion: boolean;
    /** A picture on disk that panes show, dithered, in place of the grain. */
    paneImage: string | null;
    /** How strongly the pane texture or picture shows, 1 being as designed. */
    paneStrength: number;
    terminalFontSize: number;
    chatTextScale: number;
    editorTextScale: number;
    windowOpacity: number;
    windowBlur: number;
    cloudBrowser: string;
    cloudBrowserShortcut: string;
    keybindingOverrides: KeybindingOverrides;
    sideRailOpen: boolean;
    tabRailOpen: boolean;
    sideRailWidth: number;
    tabRailWidth: number;
    diffTarget: Record<string, DiffTarget | null>;
    /** Each plugin's own settings, by plugin id, in whatever shape the plugin decodes. */
    pluginSettings: Readonly<Record<string, unknown>>;
    /** Plugins switched off in Settings; they are built in but act as if absent. */
    disabledPlugins: readonly string[];
    /** The repository and account picked by hand on each code host, by project folder, as the core keeps them. */
    codeHosts: Readonly<Record<string, Record<string, CodeHostChoice>>>;
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
    midTurnSend: MidTurnSend;
    /** The tab rail shows every CLI's chats instead of one provider's. */
    tabRailAllAgents: boolean;
    /** Providers whose chats move to the next signed-in account when one runs out of usage. */
    accountAutoSwitch: Partial<Record<AccountAgentType, boolean>>;
    tabRailScope: TabRailScope;
    tabRailPage: TabRailPage;
    pinnedChats: PinnedChat[];
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
    /** For a project folder that holds several repositories, by root path, the one its Git views open. */
    defaultGitRepos: Record<string, string>;
}

export type UpdateOperationState = "available" | "waiting" | "preparing" | "downloading" | "installing" | "restarting" | "error";

/** A chat at work that installing an update now would stop and start again,
 *  rather than carry over with its agent still running. */
export interface UpdateBlocker {
    agentId: string;
    provider: string;
    title: string | null;
    /** `starting` while its agent starts, `busy` in the middle of a request. */
    reason: string;
}

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
    /** The agent the project picker is choosing a new project for, or null when it opens projects. */
    pickerMovingAgentId: string | null;
    agentPaletteOpen: boolean;
    filePaletteOpen: boolean;
    newTabPaletteOpen: boolean;
    /** The agent whose desk has its address open in the middle of the page, from ⌘L. */
    deskAddressOpen: string | null;
    settingsOpen: boolean;
    settingsPage: SettingsPageId;
    /** The open project the Project page edits, or null to follow the project in front. */
    settingsProject: string | null;
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

    /** How the core shows each open agent, by id, as its device view last said. Never persisted. */
    agentStatus: Record<string, import("./agentStatus").AgentShown>;
    /** Questions agents asked that wait on an answer, as the core last sent them. */
    agentQuestions: import("../agents/agentQuestions").AgentQuestion[];
    /** Each project's board cards, by the project folder the core names, as the core last sent them. */
    agentBoards: Record<string, import("../agents/agentBoard").BoardCard[]>;
    /** Each project's Auto settings and how many of its agents run, by the same project folder. */
    agentBoardAuto: Record<string, import("../agents/agentBoard").BoardAuto>;
    /** The core's name for each project folder the app has opened a board for, when it differs. */
    boardProjects: Record<string, string>;

    commandPaletteOpen: boolean;
    themePaletteOpen: boolean;
    onboardingOpen: boolean;
    diagnosticsOpen: boolean;
    whatsNewOpen: boolean;
    commandPopup: { id: string; title: string; startup: string; cwd: string; context: import("./types").PtyContext } | null;
    /** What the program in each core terminal last called it, by session id. */
    terminalTitles: Record<string, string>;
    lastSessionId: string | null;

    pendingUpdate: PendingUpdate | null;
    /** The chats a waiting update waits on before it installs. */
    updateBlockers: UpdateBlocker[];
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
        buddies: false,
        buddyPack: DEFAULT_BUDDY_STYLE.pack,
        buddyShape: DEFAULT_BUDDY_STYLE.shape,
        buddyMotion: true,
        paneImage: null,
        paneStrength: 1,
        terminalFontSize: DEFAULT_TERMINAL_FONT_SIZE,
        chatTextScale: DEFAULT_CHAT_TEXT_SCALE,
        editorTextScale: DEFAULT_EDITOR_TEXT_SCALE,
        windowOpacity: 1,
        windowBlur: 0,
        cloudBrowser: "",
        cloudBrowserShortcut: "",
        keybindingOverrides: {},
        sideRailOpen: true,
        tabRailOpen: true,
        sideRailWidth: RAIL_WIDTH.start.initial,
        tabRailWidth: RAIL_WIDTH.end.initial,
        diffTarget: {},
        pluginSettings: {},
        disabledPlugins: [],
        codeHosts: {},
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
        midTurnSend: "steer",
        tabRailAllAgents: false,
        accountAutoSwitch: {},
        tabRailScope: "project",
        tabRailPage: "agents",
        pinnedChats: [],
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
        defaultGitRepos: {},

        home: "",
        pluginManifests: [],
        pickerOpen: false,
        pickerMode: "all",
        pickerMovingAgentId: null,
        agentPaletteOpen: false,
        filePaletteOpen: false,
        newTabPaletteOpen: false,
        deskAddressOpen: null,
        settingsOpen: false,
        settingsPage: "general",
        settingsProject: null,
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
        agentStatus: {},
        agentQuestions: [],
        agentBoards: {},
        agentBoardAuto: {},
        boardProjects: {},
        commandPaletteOpen: false,
        themePaletteOpen: false,
        onboardingOpen: false,
        diagnosticsOpen: false,
        whatsNewOpen: false,
        commandPopup: null,
        terminalTitles: {},
        lastSessionId: null,
        pendingUpdate: null,
        updateBlockers: [],
        lastUpdateCheck: null,
    };
});

export const getState = useStore.getState;
export const setState = useStore.setState;

export function mutate(fn: (draft: Draft<StoreState>) => void): void {
    setState((st) => produce(st, fn));
}
