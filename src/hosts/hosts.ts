import { coreCall, coreRequest } from "../api/core";
import type { CoreChange } from "../api/coreChanged";
import type { ChatInfo, DeviceView, ProjectInfo, SessionInfo } from "@shared/wire.generated";
import { invokeCommand as invoke } from "../api/invoke";
import { getIpcTransport } from "../api/transport";
import { agentLabel, isAgentType } from "@shared/agents/catalog";
import { agentWindow } from "../state/agentWindow";
import { makeWindow } from "../state/commands/shared";
import { createProjectSession, createSshSession } from "../state/commands/sessions";
import { remoteApi } from "../api/remote";
import { offerResumableSessions } from "../terminal/sessionResume";
import { agentWindowId, ownerSessionId } from "../state/selectors";
import { getState, mutate, setState, useStore, type StoreState } from "../state/store";
import { notify, reportError, swallow } from "../state/toast";
import { confirmDialog } from "../state/dialog";
import { accountApi } from "../api/account";
import type { Agent, PaneNode, Session } from "../state/types";
import type { Draft } from "immer";

/** A row of the rail's Hosts group, as the core lists it: a host linked with this one, or an SSH host a terminal is open to. */
export interface HostRow {
    /** The host's core key, which every request to it names; none for an SSH host without Sikemux. */
    readonly id: string | null;
    readonly name: string;
    readonly platform: string | null;
    /** How this host's core reaches it; `failed` only for an SSH host ssh gave up on. */
    readonly reach: "connecting" | "connected" | "away" | "failed";
    /** The SSH host it was installed on from here, or the row's own alias. */
    readonly ssh: string | null;
    /**
     * What the core found its SSH host to run when it last asked: no Sikemux, Sikemux on this host's account, signed
     * out, on another account, its core stopped, or a release too old to say which host it is.
     */
    readonly found?: "nothing" | "same" | "signedOut" | "other" | "stopped" | "outdated";
    /** What setting its SSH host up takes, by the core's word for it. */
    readonly setUp?: "install" | "update" | "signIn" | "start";
}

/** Another host on the account. */
export type HostEntry = HostRow & { readonly id: string; readonly platform: string };

export const hostsApi = {
    list: () => coreCall<HostRow[]>("hosts"),
};

/**
 * Runs `command` on the SSH host `alias` over the person's own SSH, in a terminal tab of its SSH session where a
 * password ssh or sudo asks for is typed, and resolves once the host is on the account.
 */
async function signInOverSsh(alias: string, command: "host_install" | "host_sign_in" | "host_update" | "host_start", title: string): Promise<void> {
    const controller = new AbortController();
    const done = new Promise<void>((resolve, reject) => {
        void getIpcTransport()
            .subscribe<{ alias: string; problem: string | null }>(
                "host_installed",
                ({ payload }) => {
                    if (payload.alias !== alias) return;
                    controller.abort();
                    if (payload.problem) reject(new Error(payload.problem));
                    else resolve();
                },
                { signal: controller.signal },
            )
            .catch(reject);
    });
    try {
        const id = await invoke<number>(command, { alias });
        offerResumableSessions([id]);
        createSshSession(alias);
        mutate((d) => {
            const session = d.sessions[d.activeSessionId];
            if (!session) return;
            const window = makeWindow("", title, { role: "named" });
            window.root = { ...(window.root as PaneNode), ptyId: id };
            d.windows[window.id] = window;
            d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), window.id];
            session.activeWindowId = window.id;
        });
    } catch (error) {
        controller.abort();
        throw error;
    }
    await done;
}

/** Setting an SSH host up: installing Sikemux, signing in, starting or updating the Sikemux it has. */
export type SshWork = "installing" | "signing in" | "starting" | "updating";

/** How each way of setting an SSH host up is offered, and what a row says while it runs. */
export const SSH_WORK: Readonly<Record<SshWork, { offer: string; running: string }>> = {
    installing: { offer: "Install Sikemux", running: "installing" },
    "signing in": { offer: "Sign in", running: "signing in" },
    starting: { offer: "Start Sikemux", running: "starting" },
    updating: { offer: "Update Sikemux", running: "updating" },
};

const WORK: Readonly<Record<NonNullable<HostRow["setUp"]>, SshWork>> = {
    install: "installing",
    signIn: "signing in",
    start: "starting",
    update: "updating",
};

/** What setting up a row's SSH host takes, as the core says from what it found there. */
export function setUpFor(row: HostRow | undefined): SshWork | null {
    return row?.setUp ? WORK[row.setUp] : null;
}

/** What setting up the SSH host `alias` takes; installing while no row has said what it runs. */
export function sshSetUp(state: Pick<StoreState, "hosts" | "openSshHosts">, alias: string): SshWork | null {
    const row = [...state.hosts, ...state.openSshHosts].find((candidate) => candidate.ssh === alias);
    return row ? setUpFor(row) : "installing";
}

/** What a host row says of the Sikemux found on its SSH host, when that is not running on this host's account. */
export function sshFoundLabel(row: HostRow | undefined): string | null {
    const found = row?.found;
    if (!found) return null;
    return (
        { signedOut: "not signed in", other: "on another account", stopped: "stopped", outdated: "older release", same: null, nothing: null } as const
    )[found];
}

/** Sets the SSH host `alias` up, once at a time for every place that offers it, then asks it again what it is. */
export function setUpOverSsh(alias: string, work: SshWork): void {
    if (getState().sshWork[alias]) return;
    setState({ sshWork: { ...getState().sshWork, [alias]: work } });
    const run: Record<SshWork, () => Promise<unknown>> = {
        installing: () => signInOverSsh(alias, "host_install", "Install Sikemux"),
        "signing in": () => signInOverSsh(alias, "host_sign_in", "Sign in"),
        starting: () => signInOverSsh(alias, "host_start", "Start Sikemux"),
        updating: () => signInOverSsh(alias, "host_update", "Update Sikemux"),
    };
    void run[work]()
        .catch(reportError(`${SSH_WORK[work].offer} on ${alias}`))
        .finally(() => {
            const { [alias]: _, ...others } = getState().sshWork;
            setState({ sshWork: others });
        });
}

/** The Install Sikemux pane for the SSH host `alias`, a tab of its SSH session (ADR 0019). */
export function openInstallPane(alias: string): void {
    createSshSession(alias);
    mutate((d) => {
        const session = d.sessions[d.activeSessionId];
        if (session?.kind !== "ssh") return;
        const open = (d.windowsBySession[session.id] ?? []).find((id) => d.windows[id]?.role === "install");
        if (open) {
            session.activeWindowId = open;
            return;
        }
        const window = makeWindow("", "Install Sikemux", { kind: "install-sikemux", role: "install" });
        d.windows[window.id] = window;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), window.id];
        session.activeWindowId = window.id;
    });
}

/** A project on another host, as that host shows it. */
export interface HostProject {
    readonly host: HostEntry;
    readonly project: ProjectInfo;
    readonly view: DeviceView;
}

const REMOTE_SESSION_PREFIX = "host:";

function remoteSessionId(host: string, project: string): string {
    return `${REMOTE_SESSION_PREFIX}${host}:${project}`;
}

function inFolder(path: string | null | undefined, folder: string): boolean {
    return !!path && (path === folder || path.startsWith(folder.endsWith("/") ? folder : `${folder}/`));
}

/** The project a session on another host is, as that host shows it. */
export function hostProjectOf(state: Pick<StoreState, "hosts" | "hostViews">, session: Session): HostProject | undefined {
    const host = session.kind === "project" && session.host ? state.hosts.find((candidate) => candidate.id === session.host) : undefined;
    const view = host && state.hostViews[host.id];
    const project = view?.workspace.projects.find((candidate) => candidate.path === session.cwd);
    return host && view && project ? { host, project, view } : undefined;
}

/** The chats running in a project on another host, newest attention first. */
export function chatsIn(place: HostProject): ChatInfo[] {
    return place.view.chats.filter((chat) => inFolder(chat.cwd, place.project.path));
}

/** The plain terminals running in a project on another host. */
export function terminalsIn(place: HostProject): SessionInfo[] {
    return place.view.sessions.filter(
        (session) => session.kind === "terminal" && session.running && !session.agentId && inFolder(session.project, place.project.path),
    );
}

/** The project a new chat on `host` starts in: `session` when it is one of that host's, else the host's first. */
export function projectOnHost(
    state: Pick<StoreState, "hosts" | "hostViews" | "sessionOrder" | "sessions">,
    host: string,
    session: Session | undefined,
): { session: Session; project: HostProject } | undefined {
    const candidates = [session, ...state.sessionOrder.map((id) => state.sessions[id])];
    for (const candidate of candidates) {
        const project = candidate?.host === host ? hostProjectOf(state, candidate) : undefined;
        if (candidate && project) return { session: candidate, project };
    }
    return undefined;
}

/** A host the side rail chooses: this one or one on the account, by its core's key (none for this one), or an SSH host by its alias. */
export type HostChoice = { readonly kind: "core"; readonly host?: string } | { readonly kind: "ssh"; readonly alias: string };

export const THIS_HOST: HostChoice = { kind: "core" };

export function sameHost(a: HostChoice, b: HostChoice): boolean {
    return a.kind === "core" ? b.kind === "core" && a.host === b.host : b.kind === "ssh" && a.alias === b.alias;
}

/** What the rail keeps per host, such as the session last in front there. */
function hostKey(choice: HostChoice): string {
    return choice.kind === "core" ? `core:${choice.host ?? ""}` : `ssh:${choice.alias}`;
}

/** The host a session belongs to; none for one, such as a plugin's, that runs on no host in particular. */
function hostOfSession(session: Session): HostChoice | undefined {
    if (session.kind === "ssh") return { kind: "ssh", alias: session.name };
    if (session.kind === "project" || session.kind === "command") return hostOf(session.host);
    return undefined;
}

/** The first session open on `choice`, its projects before anything else. */
function firstOn(state: Pick<StoreState, "sessionOrder" | "sessions">, choice: HostChoice): string | undefined {
    const on = state.sessionOrder.filter((id) => {
        const host = state.sessions[id] && hostOfSession(state.sessions[id]);
        return !!host && sameHost(host, choice);
    });
    return on.find((id) => state.sessions[id]?.kind === "project") ?? on[0];
}

/**
 * Chooses a host under Hosts, and puts in front what was last in front there, else its first project, else nothing:
 * each host has its own projects (ADR 0019).
 */
export function chooseHost(choice: HostChoice): void {
    const state = getState();
    const last = state.lastOnHost[hostKey(choice)];
    const back = last && state.sessions[last] ? last : firstOn(state, choice);
    // An SSH host without Sikemux is a shell: choosing it shows a terminal there, opening one when none is.
    if (choice.kind === "ssh" && !back) return createSshSession(choice.alias);
    mutate((d) => {
        d.chosenHost = choice;
        d.activeSessionId = back ?? "";
        d.zoomedPaneId = null;
        d.settingsOpen = false;
        d.pickerOpen = false;
    });
    if (choice.kind === "ssh" && back && (state.windowsBySession[back] ?? []).length === 0) newSshTerminalOn(choice.alias);
}

/** The chosen host follows the session in front, which that host then keeps as the one last in front there. */
function followSessionInFront(): void {
    const { activeSessionId, sessions, chosenHost, lastOnHost } = getState();
    const session = sessions[activeSessionId];
    const host = session && hostOfSession(session);
    if (!host) return;
    const key = hostKey(host);
    if (sameHost(host, chosenHost) && lastOnHost[key] === activeSessionId) return;
    setState({ chosenHost: sameHost(host, chosenHost) ? chosenHost : host, lastOnHost: { ...lastOnHost, [key]: activeSessionId } });
}

/** Brings `sessionId` forward. */
export function bringForward(d: Draft<StoreState>, sessionId: string): void {
    d.activeSessionId = sessionId;
    d.zoomedPaneId = null;
}

/** Where new chats and terminals start, which its row under Hosts shows chosen. */
export function newWorkHost(state: Pick<StoreState, "chosenHost">): HostChoice {
    return state.chosenHost;
}

/** The host a chat or terminal tab runs on, as the Hosts group chooses it. */
export function hostOf(host: string | undefined): HostChoice {
    return host ? { kind: "core", host } : THIS_HOST;
}

/** The chosen host while its row is there, else this host: an account host taken off it, or an SSH host closed. */
function settleChosenHost(): void {
    const { chosenHost, hosts, sessions } = getState();
    const installed = chosenHost.kind === "ssh" && hosts.find((host) => host.ssh === chosenHost.alias);
    if (installed) return chooseHost({ kind: "core", host: installed.id });
    const there =
        chosenHost.kind === "core"
            ? !chosenHost.host || hosts.some((host) => host.id === chosenHost.host)
            : Object.values(sessions).some((session) => session.kind === "ssh" && session.name === chosenHost.alias);
    if (!there) chooseHost(THIS_HOST);
}

/** Another login on the SSH host `alias`, as a tab of its session, which its row under Hosts means is open. */
export function newSshTerminalOn(alias: string): void {
    mutate((d) => {
        const session = d.sessionOrder.map((id) => d.sessions[id]).find((candidate) => candidate?.kind === "ssh" && candidate.name === alias);
        if (!session) return;
        const window = makeWindow("", alias, { role: "named" });
        d.windows[window.id] = window;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), window.id];
        session.activeWindowId = window.id;
        bringForward(d, session.id);
    });
}

/** Shows a tab that is open already, from a chosen host's list. */
export function showTab(windowId: string): void {
    mutate((d) => {
        const sessionId = ownerSessionId(d, windowId);
        if (!sessionId) return;
        d.sessions[sessionId].activeWindowId = windowId;
        bringForward(d, sessionId);
    });
}

/** The session a host keeps for its terminals and chats outside any project open here, made the first time. */
function hostSession(d: Draft<StoreState>, host: string): Draft<Session> {
    const id = `${REMOTE_SESSION_PREFIX}${host}`;
    if (!d.sessions[id]) {
        d.sessions[id] = { id, name: hostName(d, host) ?? host, kind: "command", cwd: "", pinned: false, activeWindowId: "", host };
        d.sessionOrder.push(id);
        d.windowsBySession[id] = [];
    }
    return d.sessions[id];
}

/** The project on `host` that holds `path`, and the session open here for it, if any. */
export function placeOn(
    state: Pick<StoreState, "hostViews" | "sessions">,
    host: string,
    path: string | null | undefined,
): { project: ProjectInfo | undefined; sessionId: string | null } {
    const project = state.hostViews[host]?.workspace.projects.find((candidate) => inFolder(path, candidate.path));
    const sessionId = project ? remoteSessionId(host, project.id) : null;
    return { project, sessionId: sessionId && state.sessions[sessionId] ? sessionId : null };
}

/** Opens a terminal on another host outside any project, in the session that host keeps. */
export function openHostTerminal(host: string): void {
    newRemoteTerminal(null, host, "");
}

/**
 * The lock of ADR 0018 on a host, this one when there is none, as a menu item that flips it. Read when the menu opens,
 * since only that host knows it.
 */
export async function approvalItem(host?: string): Promise<{ label: string; selected: boolean; disabled: boolean; run: () => void }> {
    const label = "Require approval for new devices";
    const read = await coreCall<{ requireApproval: boolean }>("requireApproval", null, host).catch(() => null);
    if (!read) return { label, selected: false, disabled: true, run: () => {} };
    return {
        label,
        selected: read.requireApproval,
        disabled: false,
        run: () => void coreCall("setRequireApproval", { requireApproval: !read.requireApproval }, host).catch(reportError(label)),
    };
}

/** What the rail calls a host. */
export function hostTitle(state: Pick<StoreState, "hosts">, choice: HostChoice): string {
    if (choice.kind === "ssh") return choice.alias;
    return hostName(state, choice.host) ?? "This host";
}

export function hostName(state: Pick<StoreState, "hosts">, id: string | undefined): string | undefined {
    return id ? (state.hosts.find((host) => host.id === id)?.name ?? "another host") : undefined;
}

/**
 * Gives every project another host shows a session of its own, listed under that host (ADR 0019), and drops those of
 * hosts the app left or projects they closed. They stay out of this host's core's workspace.
 */
function placeRemoteProjects(): void {
    const state = getState();
    const sessions = Object.values(state.sessions);
    const wanted = new Map<string, { host: string; project: ProjectInfo }>();
    // A host whose view has not come yet keeps the rows it had.
    const unseen = new Set<string>();
    for (const host of state.hosts) {
        const view = state.hostViews[host.id];
        if (!view) {
            unseen.add(host.id);
            continue;
        }
        for (const project of view.workspace.projects) wanted.set(remoteSessionId(host.id, project.id), { host: host.id, project });
    }
    const stale = sessions.filter((session) => session.kind === "project" && session.host && !unseen.has(session.host) && !wanted.has(session.id));
    const missing = [...wanted].filter(([id]) => !state.sessions[id]);
    const renamed = [...wanted].filter(([id, { project }]) => state.sessions[id] && state.sessions[id].name !== project.name);
    if (stale.length === 0 && missing.length === 0 && renamed.length === 0) return;
    mutate((d) => {
        for (const session of stale) {
            for (const windowId of d.windowsBySession[session.id] ?? []) delete d.windows[windowId];
            delete d.windowsBySession[session.id];
            delete d.sessions[session.id];
            d.sessionOrder = d.sessionOrder.filter((id) => id !== session.id);
            if (d.activeSessionId === session.id) d.activeSessionId = "";
        }
        for (const [id, { host, project }] of missing) {
            d.sessions[id] = { id, name: project.name, kind: "project", cwd: project.path, pinned: false, activeWindowId: "", host };
            d.sessionOrder.push(id);
            d.windowsBySession[id] = [];
        }
        for (const [id, { project }] of renamed) d.sessions[id].name = project.name;
    });
}

function remoteTerminalWindow(host: string, cwd: string) {
    const window = makeWindow(cwd, "Terminal", { role: "term" });
    window.root = { ...(window.root as PaneNode), host };
    return window;
}

/** The session `sessionId` names, or the one `host` keeps for what runs outside a project when it is null. */
function sessionIn(d: Draft<StoreState>, sessionId: string | null, host: string): Draft<Session> | undefined {
    return sessionId === null ? hostSession(d, host) : d.sessions[sessionId];
}

/** Opens a terminal on `host` in `cwd`, a folder there, as a tab of `sessionId`, or of the host's own session when null. */
export function newRemoteTerminal(sessionId: string | null, host: string, cwd: string): void {
    mutate((d) => {
        const session = sessionIn(d, sessionId, host);
        if (!session) return;
        const window = remoteTerminalWindow(host, cwd);
        d.windows[window.id] = window;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), window.id];
        session.activeWindowId = window.id;
        bringForward(d, session.id);
    });
}

/** Shows a terminal already running on `host` as a tab of `sessionId`, or of the host's own session when null. */
export function openRemoteTerminal(sessionId: string | null, host: string, terminal: SessionInfo, cwd: string): void {
    const open = Object.values(getState().windows).find((window) => window.root.type === "pane" && window.root.ptyId === terminal.id);
    if (open) return showTab(open.id);
    mutate((d) => {
        const session = sessionIn(d, sessionId, host);
        if (!session) return;
        const window = remoteTerminalWindow(host, cwd);
        window.root = { ...(window.root as PaneNode), ptyId: terminal.id };
        d.windows[window.id] = window;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), window.id];
        session.activeWindowId = window.id;
        bringForward(d, session.id);
    });
}

/** Shows a chat running on `host` as a tab of `sessionId`, or of the host's own session when null, taking it up there. */
export function openRemoteChat(
    sessionId: string | null,
    host: string,
    chat: Pick<ChatInfo, "agentId" | "provider" | "title" | "name" | "cwd">,
): void {
    if (!isAgentType(chat.provider)) {
        notify("info", `This app can't show ${chat.provider} chats yet`);
        return;
    }
    const type = chat.provider;
    const open = agentWindowId(getState(), chat.agentId);
    if (open) return showTab(open);
    mutate((d) => {
        const session = sessionIn(d, sessionId, host);
        if (!session) return;
        const agent: Agent = {
            id: chat.agentId,
            type,
            title: chat.title ?? agentLabel(type),
            name: chat.name ?? undefined,
            createdAt: Date.now(),
            permissionMode: d.defaultAgentPermissionMode,
            cwd: chat.cwd,
            launchState: "live",
            host,
        };
        d.agents[agent.id] = agent;
        const window = agentWindow(agent, chat.cwd);
        d.windows[window.id] = window;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), window.id];
        session.activeWindowId = window.id;
        bringForward(d, session.id);
    });
}

/** Starts a chat with `type` in a copy on another host, on that host's own sign-in, and opens it here. */
export async function startRemoteChat(sessionId: string, place: HostProject, type: string): Promise<void> {
    const launcher = place.view.workspace.launchers.find((candidate) => candidate.provider === type);
    if (!launcher) {
        notify("info", `${agentLabel(type)} isn't set up on ${place.host.name}`);
        return;
    }
    const begun = await coreRequest<{ agentId: string }>(
        { op: "startChat", launcher: launcher.id, project: place.project.id, permissionMode: null, model: null, effort: null },
        place.host.id,
    );
    openRemoteChat(sessionId, place.host.id, { agentId: begun.agentId, provider: type, title: null, name: null, cwd: place.project.path });
}

/** How long the app waits for a host to show a project it was asked to open, after it answered. */
const OPENED_WAIT_MS = 30_000;

/** A Sikemux host a project can be opened on: one on the account by its key, or this one. */
export interface OpenTarget {
    readonly id: string | undefined;
    readonly name: string;
}

/**
 * Opens `session`'s project on the Sikemux host `target` (ADR 0019): that host opens a checkout it has of the same
 * repository, or clones it from its remote with its own sign-in, or takes it from the project's host when it cannot.
 * Only committed work goes; uncommitted changes stay where they are. Then the person is on that host, in that project.
 */
export async function openOnHost(session: Session, target: OpenTarget): Promise<void> {
    const confirmed = await confirmDialog({
        title: `Open ${session.name} on ${target.name}?`,
        body: `${target.name} clones it from its remote, or takes it from where it is when it cannot reach the remote. Only what is committed goes over; uncommitted changes stay there.`,
        confirmLabel: "Open",
    });
    if (!confirmed) return;
    notify("info", `Opening ${session.name} on ${target.name}…`);
    const key = target.id ?? (await remoteApi.status()).coreId;
    const opened = await coreCall<{ project: string; path: string }>("openOnHost", { path: session.cwd, host: key }, session.host);
    if (!target.id) return createProjectSession(opened.path);
    const host = target.id;
    const id = remoteSessionId(host, opened.project);
    const shown = await new Promise<boolean>((resolve) => {
        if (getState().sessions[id]) return resolve(true);
        const timer = window.setTimeout(() => {
            stop();
            resolve(false);
        }, OPENED_WAIT_MS);
        const stop = useStore.subscribe((state) => {
            if (!state.sessions[id]) return;
            window.clearTimeout(timer);
            stop();
            resolve(true);
        });
    });
    if (!shown) throw new Error(`${target.name} opened it but has not shown it here yet`);
    mutate((d) => bringForward(d, id));
}

/** Says when a device or host joins the account, with one tap to take it off again (ADR 0018). */
function noticeNewDevices(signal: AbortSignal): void {
    void getIpcTransport()
        .subscribe<{ key: string; name: string }>(
            "account_device",
            ({ payload }) =>
                notify("info", `New device on your account: ${payload.name || "a device"}`, {
                    timeoutMs: null,
                    action: {
                        label: "Remove",
                        run: () => accountApi.removeDevice(payload.key).catch(reportError(`Remove ${payload.name || "the device"}`)),
                    },
                }),
            { signal },
        )
        .catch(swallow("follow new devices on the account"));
}

/** Keeps the store's hosts and their views as the app reaches them. */
export function startHosts(): () => void {
    const controller = new AbortController();
    const transport = getIpcTransport();
    noticeNewDevices(controller.signal);
    const front = getState().sessions[getState().activeSessionId];
    const frontHost = front && hostOfSession(front);
    if (frontHost && !sameHost(frontHost, THIS_HOST)) chooseHost(THIS_HOST);
    else followSessionInFront();
    const setHosts = (rows: HostRow[]) => {
        const hosts = rows.filter((row): row is HostEntry => row.id !== null);
        const ids = new Set(hosts.map((host) => host.id));
        setState({
            hosts,
            openSshHosts: rows.filter((row) => row.id === null),
            hostViews: Object.fromEntries(Object.entries(getState().hostViews).filter(([id]) => ids.has(id))),
        });
        placeRemoteProjects();
    };
    void transport
        .subscribe<CoreChange>(
            "core_changed",
            ({ payload }) => {
                if (payload.topic === "hosts") void hostsApi.list().then(setHosts).catch(swallow("list other hosts"));
            },
            { signal: controller.signal },
        )
        .catch(swallow("follow other hosts"));
    void transport
        .subscribe<{ host: string; view: DeviceView }>(
            "host_view",
            ({ payload }) => {
                setState({ hostViews: { ...getState().hostViews, [payload.host]: payload.view } });
                placeRemoteProjects();
            },
            { signal: controller.signal },
        )
        .catch(swallow("follow other hosts' views"));
    void hostsApi.list().then(setHosts).catch(swallow("list other hosts"));
    const stopFollowing = useStore.subscribe((state, previous) => {
        if (state.activeSessionId !== previous.activeSessionId || state.sessions !== previous.sessions) followSessionInFront();
        if (state.hosts !== previous.hosts || state.sessions !== previous.sessions) settleChosenHost();
    });
    return () => {
        controller.abort();
        stopFollowing();
    };
}
