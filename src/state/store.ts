import type { CodeHostChoice, Preferences } from "@shared/wire.generated";
import type { PluginManifest } from "../api/plugins";
import { create } from "zustand";
import { enableMapSet, produce, type Draft } from "immer";
import type { SettingsPageId } from "../settings/settingsIndex";
import { RAIL_WIDTH } from "../lib/railWidths";
import { DEFAULT_BUDDY_STYLE } from "@shared/buddies/style";
import { defaultProviderProfiles, defaultProviderProfileSelection } from "./types";

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
    ProjectSpace,
    ProviderProfile,
    ProviderProfileSelection,
    RecentEntry,
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
    /** Agents show a character in place of their harness logo. */
    /** How agents' faces look, as the core keeps it. */
    faceStyle: import("@shared/wire.generated").FaceStyle;
    /** The face each agent the core lists wears, or null while it shows its CLI's logo, by agent id. */
    faces: Readonly<Record<string, import("@shared/wire.generated").AgentFace | null>>;
    sideRailOpen: boolean;
    tabRailOpen: boolean;
    sideRailWidth: number;
    tabRailWidth: number;
    diffTarget: Record<string, DiffTarget | null>;
    /** Plugins switched off in Settings; they are built in but act as if absent. */
    disabledPlugins: readonly string[];
    /** The repository and account picked by hand on each code host, by project folder, as the core keeps them. */
    codeHosts: Readonly<Record<string, Record<string, CodeHostChoice>>>;
    spaces: readonly ProjectSpace[];
    /** The space id each project belongs to, by project folder, so it outlives closing the project. */
    projectSpaces: Readonly<Record<string, string>>;
    /** The space the rail shows, or null for every project. */
    activeSpaceId: string | null;
    /** How the person wants Sikemux to behave, as the core keeps it; null until read. */
    preferences: Preferences | null;
    /** The tab rail shows every CLI's chats instead of one provider's. */
    tabRailAllAgents: boolean;
    /** Providers whose chats move to the next signed-in account when one runs out of usage. */
    accountAutoSwitch: Partial<Record<AgentType, boolean>>;
    tabRailScope: TabRailScope;
    tabRailPage: TabRailPage;
    pinnedChats: PinnedChat[];
    lastReleaseNotes: HeldRelease | null;
    recentCommandKeys: string[];
    /** Non-secret launch profiles and the per-agent defaults that reference them. */
    providerProfiles: ProviderProfile[];
    selectedProviderProfileIds: ProviderProfileSelection;
    defaultAgentPermissionMode: AgentPermissionMode;
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
    agentStatus: Record<string, import("@shared/wire.generated").AgentShown>;
    /** How each project's open agents show together, by project id, as the core last said. Never persisted. */
    projectStatus: Record<string, import("@shared/wire.generated").AgentRollup>;
    /** Questions agents asked that wait on an answer, as the core last sent them. */
    agentQuestions: import("../api/agentQuestions").AgentQuestion[];
    /** Each project's board cards, by the project folder the core names, as the core last sent them. */
    agentBoards: Record<string, import("@shared/agents/agentBoard").BoardCard[]>;
    /** Each project's Auto settings and how many of its agents run, by the same project folder. */
    agentBoardAuto: Record<string, import("@shared/agents/agentBoard").BoardAuto>;
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
    /** The account's other hosts this app joined, and how each is reached. */
    hosts: readonly import("../hosts/hosts").HostEntry[];
    /** The SSH hosts a terminal is open to, without Sikemux on them from here, as the core lists them. */
    openSshHosts: readonly import("../hosts/hosts").HostRow[];
    /** What each of those hosts last showed this app, by its id. */
    hostViews: Readonly<Record<string, import("@shared/wire.generated").DeviceView>>;
    /** The SSH hosts being set up now, by alias. */
    sshWork: Readonly<Record<string, import("../hosts/hosts").SshWork>>;
    /** The host chosen under Hosts, beside the chosen project: where the tab rail looks and new work starts. */
    chosenHost: import("../hosts/hosts").HostChoice;
    /** The session last in front on each host, which choosing that host again puts back. */
    lastOnHost: Readonly<Record<string, string>>;

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
        faceStyle: { on: false, pack: DEFAULT_BUDDY_STYLE.pack, shape: DEFAULT_BUDDY_STYLE.shape, motion: true },
        faces: {},
        sideRailOpen: true,
        tabRailOpen: true,
        sideRailWidth: RAIL_WIDTH.start.initial,
        tabRailWidth: RAIL_WIDTH.end.initial,
        diffTarget: {},
        disabledPlugins: [],
        codeHosts: {},
        spaces: [],
        projectSpaces: {},
        activeSpaceId: null,
        preferences: null,
        tabRailAllAgents: false,
        accountAutoSwitch: {},
        tabRailScope: "project",
        tabRailPage: "agents",
        pinnedChats: [],
        lastReleaseNotes: null,
        recentCommandKeys: [],
        providerProfiles: defaultProviderProfiles(),
        selectedProviderProfileIds: defaultProviderProfileSelection(),
        defaultAgentPermissionMode: "bypass",

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
        projectStatus: {},
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
        hosts: [],
        openSshHosts: [],
        hostViews: {},
        chosenHost: { kind: "core" },
        lastOnHost: {},
        sshWork: {},
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
