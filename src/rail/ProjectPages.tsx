import { type MouseEvent, type ReactNode, useState } from "react";
import { useShortcutLabel } from "../commands/useShortcutLabel";
import { basename, dirname, relativePath } from "../lib/paths";
import * as cmd from "../state/commands";
import { collectPanes } from "../state/layout";
import { activeTabRef, documentsOf, terminalLabel } from "../state/selectors";
import { getState, useStore } from "../state/store";
import type { Session, TabRef } from "../state/types";
import { FileIcon } from "../ui/FileIcon";
import { IconClose, IconFile, IconPlus, IconTerminal } from "../ui/Icons";
import { Panel } from "../ui/Panel";
import { Tooltip } from "../ui/Tooltip";
import { fileTabMenu, windowTabMenu } from "../workspace/tabMenus";
import { AgentRowProject } from "./AgentRowTree";
import { type CtxItem, FileTree, TreeContextMenu } from "./FileTree";
import { RailEmpty } from "./RailEmpty";
import { NewRow } from "./RailNewRow";
import { RailToggle } from "./RailToggle";
import { ScopeTabs } from "./ScopeTabs";

const EMPTY_IDS: readonly string[] = [];
const openEntry = (entry: { path: string }) => cmd.requestOpenFile(entry.path);

type Menu = { x: number; y: number; items: CtxItem[] };

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

/** A tab of the session's strip, as a rail row: it selects the tab, closes it, and opens the strip's menu for it. */
function TabRow({
    tab,
    icon,
    glyph,
    label,
    detail,
    title,
    project,
    active,
    onMenu,
}: {
    tab: TabRef;
    icon: ReactNode;
    /** What colours the icon, the way the strip colours the same tab's. */
    glyph?: string;
    label: string;
    detail?: string | null;
    title?: string;
    /** The project the tab lives in, shown under its label when the list spans projects. */
    project?: { name: string; sessionId: string };
    active: boolean;
    onMenu: (event: MouseEvent) => void;
}) {
    return (
        <div className="agent-row-wrap">
            <button
                type="button"
                className={`agent-row rail-tab${project ? " two-line" : ""}${active ? " active" : ""}`}
                title={title}
                onClick={() => {
                    if (project && project.sessionId !== getState().activeSessionId) cmd.selectSession(project.sessionId);
                    cmd.selectTab(tab);
                }}
                onAuxClick={(event) => {
                    if (event.button === 1) cmd.closeTab(tab);
                }}
                onContextMenu={onMenu}>
                <span className={`agent-glyph${glyph ? ` ${glyph}` : ""}`}>{icon}</span>
                {project ? (
                    <span className="agent-row-text">
                        <span className="agent-title">{label}</span>
                        <AgentRowProject name={project.name} />
                    </span>
                ) : (
                    <span className="agent-title">{label}</span>
                )}
                {detail && <span className="rail-file-folder">{detail}</span>}
            </button>
            <Tooltip label={`Close ${label}`}>
                <button type="button" className="row-x" aria-label={`Close ${label}`} onClick={() => cmd.closeTab(tab)}>
                    <IconClose size={11} />
                </button>
            </Tooltip>
        </div>
    );
}

function useTabMenu() {
    const [menu, setMenu] = useState<Menu | null>(null);
    const open = (event: MouseEvent, items: CtxItem[]) => {
        event.preventDefault();
        event.stopPropagation();
        setMenu({ x: event.clientX, y: event.clientY, items });
    };
    const shown = menu && <TreeContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />;
    return { open, shown };
}

/** The terminal tabs of this project, or of every open project. */
export function TerminalsPage() {
    const session = useProjectSession();
    const scope = useStore((s) => s.tabRailScope);
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const windows = useStore((s) => s.windows);
    const titles = useStore((s) => s.terminalTitles);
    const closeShortcut = useShortcutLabel("pane.close");
    const menu = useTabMenu();

    const shown = scope === "all" ? sessionOrder.map((id) => sessions[id]).filter((s) => s?.kind === "project") : session ? [session] : [];
    const tabs = shown.flatMap((owner) =>
        (windowsBySession[owner.id] ?? EMPTY_IDS).flatMap((id) => {
            const win = windows[id];
            const home = win?.role === "term" ? collectPanes(win.root).find((pane) => pane.kind === "terminal") : undefined;
            return win && home ? [{ win, owner, label: terminalLabel(win, home, titles) }] : [];
        }),
    );

    return (
        <>
            <PageHeader label="Terminals" />
            {!session ? (
                <RailEmpty icon={<IconTerminal size={20} />} title="No project open" shortcut="project.open" doing="open one" />
            ) : (
                <>
                    <div className="tab-rail-top">
                        <div className="agent-actions">
                            <NewRow icon={<IconPlus size={15} />} title="New terminal" shortcut="terminal.new" onClick={cmd.newTerminal} />
                        </div>
                        <ScopeTabs scope={scope} projectName={session.name} onChange={cmd.setTabRailScope} />
                    </div>
                    <div className="rail-scroll">
                        {tabs.length === 0 ? (
                            <RailEmpty icon={<IconTerminal size={20} />} title="No terminals open" shortcut="terminal.new" doing="start one" />
                        ) : (
                            <Panel variant="group" className="agent-group">
                                {tabs.map(({ win, owner, label }) => (
                                    <TabRow
                                        key={win.id}
                                        tab={{ id: win.id }}
                                        icon={<IconTerminal size={15} />}
                                        glyph="term"
                                        label={label}
                                        project={scope === "all" ? { name: owner.name, sessionId: owner.id } : undefined}
                                        active={owner.id === session.id && session.activeWindowId === win.id}
                                        onMenu={(event) =>
                                            menu.open(event, windowTabMenu(owner.id, getState().windows[win.id] ?? win, closeShortcut))
                                        }
                                    />
                                ))}
                            </Panel>
                        )}
                    </div>
                </>
            )}
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

    const cwd = session.cwd;
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
            <FileTree cwd={cwd} activePath={activePath} onOpenFile={openEntry} onKeepFile={openEntry} active={showing} lead={lead} />
            {menu.shown}
        </>
    );
}
