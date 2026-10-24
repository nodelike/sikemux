import { useShortcutLabel } from "../commands/useShortcutLabel";
import { basename, dirname, relativePath } from "../lib/paths";
import * as cmd from "../state/commands";
import { collectPanes } from "../state/layout";
import { activeTabRef, documentsOf, terminalLabel } from "../state/selectors";
import { getState, useStore, type StoreState } from "../state/store";
import type { FilesPlace } from "../state/commands/shared";
import type { Session } from "../state/types";
import { FileIcon } from "../ui/FileIcon";
import { IconFile, IconPlus, IconTerminal } from "../ui/Icons";
import { Panel } from "../ui/Panel";
import { fileTabMenu, windowTabMenu } from "../workspace/tabMenus";
import { FileTree } from "./FileTree";
import { RailEmpty } from "./RailEmpty";
import { NewRow } from "./RailNewRow";
import { RailToggle } from "./RailToggle";
import { useSessionName } from "../terminal/railNames";
import { NewOnHost, remoteTerminalRows, useHostProject } from "./RemoteRows";
import { ScopeTabs } from "./ScopeTabs";
import { HostTerminals } from "./HostLists";
import { TabRow, useTabMenu } from "./TabRow";
import { hostName } from "../hosts/hosts";

const EMPTY_IDS: readonly string[] = [];

/** The copy the Files page shows: the one the focused chat, terminal or editor works in on another host, else the project's own. */
function filesPlace(state: StoreState, session: Session | undefined): FilesPlace | undefined {
    if (!session) return undefined;
    const win = state.windows[session.activeWindowId];
    const pane = win && collectPanes(win.root).find((candidate) => candidate.id === win.activePaneId);
    const agent = pane && state.agents[pane.id];
    const host = pane?.host ?? agent?.host;
    return pane && host !== undefined ? { cwd: agent?.cwd ?? pane.cwd, host } : { cwd: session.cwd, host: session.host };
}

function PageHeader({ label }: { label: string }) {
    return (
        <div className="agent-header">
            <div className="agent-header-top">
                <span className="agent-header-label">{label}</span>
                <div className="agent-header-actions">
                    <RailToggle edge="end" />
                </div>
            </div>
        </div>
    );
}

function useProjectSession(): Session | undefined {
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    return session?.kind === "project" ? session : undefined;
}

/** The terminal tabs of this project, or of every open project. */
export function TerminalsPage() {
    const session = useProjectSession();
    const sessionName = useSessionName(session);
    const host = useStore((s) => s.chosenHost);
    const remoteHost = host.kind === "core" ? host.host : undefined;
    const hostLabel = useStore((s) => hostName(s, remoteHost));
    const scope = useStore((s) => s.tabRailScope);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const windows = useStore((s) => s.windows);
    const titles = useStore((s) => s.terminalTitles);
    const closeShortcut = useShortcutLabel("pane.close");
    const menu = useTabMenu();
    const place = useHostProject(session);

    if (!session) {
        return (
            <>
                <PageHeader label="Terminals" />
                <RailEmpty icon={<IconTerminal size={20} />} title="No project open" shortcut="project.open" doing="open one" />
            </>
        );
    }

    const tabs = (windowsBySession[session.id] ?? EMPTY_IDS).flatMap((id) => {
        const win = windows[id];
        const home = win?.role === "term" ? collectPanes(win.root).find((pane) => pane.kind === "terminal") : undefined;
        return win && home && home.host === remoteHost ? [{ win, home, label: terminalLabel(win, home, titles) }] : [];
    });
    const remoteTerminals = remoteTerminalRows({
        sessionId: session.id,
        place: place?.host.id === remoteHost ? place : undefined,
        open: new Set(tabs.flatMap(({ home }) => home.ptyId ?? [])),
    });

    return (
        <>
            <PageHeader label="Terminals" />
            <div className="tab-rail-top">
                <div className="agent-actions">
                    <NewOnHost />
                    <NewRow icon={<IconPlus size={15} />} title="New terminal" shortcut="terminal.new" onClick={cmd.newTerminal} />
                </div>
                <ScopeTabs scope={scope} name={sessionName} onChange={cmd.setTabRailScope} />
            </div>
            <div className="rail-scroll">
                {scope === "all" || host.kind === "ssh" ? (
                    <HostTerminals choice={host} />
                ) : tabs.length === 0 && remoteTerminals.length === 0 ? (
                    <RailEmpty
                        icon={<IconTerminal size={20} />}
                        title={hostLabel ? `No terminals in ${sessionName} on ${hostLabel}` : "No terminals open"}
                        shortcut="terminal.new"
                        doing="start one"
                    />
                ) : (
                    <Panel variant="group" className="agent-group">
                        {tabs.map(({ win, label }) => (
                            <TabRow
                                key={win.id}
                                tab={{ id: win.id }}
                                icon={<IconTerminal size={15} />}
                                glyph="term"
                                label={label}
                                active={session.activeWindowId === win.id}
                                onMenu={(event) => menu.open(event, windowTabMenu(session.id, getState().windows[win.id] ?? win, closeShortcut))}
                            />
                        ))}
                        {remoteTerminals}
                    </Panel>
                )}
            </div>
            {menu.shown}
        </>
    );
}

/** The files open in the project's editor, then the project's tree, as one list. */
export function FilesPage() {
    const session = useProjectSession();
    const showing = useStore((s) => s.tabRailPage === "files" && s.tabRailOpen);
    const windowIds = useStore((s) => (session ? s.windowsBySession[session.id] : undefined) ?? EMPTY_IDS);
    const windows = useStore((s) => s.windows);
    const editorViews = useStore((s) => s.editorViews);
    const host = useStore((s) => filesPlace(s, session)?.host);
    const cwd = useStore((s) => filesPlace(s, session)?.cwd ?? "");
    const closeShortcut = useShortcutLabel("pane.close");
    const menu = useTabMenu();

    if (!session) {
        return (
            <>
                <PageHeader label="Files" />
                <RailEmpty icon={<IconFile size={20} />} title="No project open" shortcut="project.open" doing="open one" />
            </>
        );
    }

    const openEntry = (entry: { path: string }) => cmd.requestOpenFile(entry.path, undefined, undefined, { cwd, host });
    const live = activeTabRef(session, windows, editorViews);
    const open: { id: string; path: string }[] = [];
    let activePath: string | null = null;
    for (const id of windowIds) {
        const win = windows[id];
        if (win?.role !== "files") continue;
        const documents = documentsOf(win, editorViews);
        activePath ??= documents?.activeId ?? null;
        for (const path of documents?.ids ?? EMPTY_IDS) open.push({ id, path });
    }

    const lead =
        open.length > 0 ? (
            <Panel variant="group" className="agent-group rail-open-files">
                {open.map(({ id, path }) => (
                    <TabRow
                        key={`${id}:${path}`}
                        tab={{ id, doc: path }}
                        icon={<FileIcon name={basename(path)} size={15} />}
                        label={basename(path)}
                        detail={relativePath(dirname(path), cwd)}
                        title={relativePath(path, cwd) ?? path}
                        active={live?.id === id && live.doc === path}
                        onMenu={(event) => {
                            const win = getState().windows[id];
                            if (win) menu.open(event, fileTabMenu(cwd, win, path, closeShortcut));
                        }}
                    />
                ))}
            </Panel>
        ) : undefined;

    return (
        <>
            <PageHeader label="Files" />
            <FileTree cwd={cwd} host={host} activePath={activePath} onOpenFile={openEntry} onKeepFile={openEntry} active={showing} lead={lead} />
            {menu.shown}
        </>
    );
}
