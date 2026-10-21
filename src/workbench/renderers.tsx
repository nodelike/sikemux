import { lazy, Suspense, type ReactNode } from "react";
import type { CorePaneKind, PaneNode, Session, Window as WindowT } from "../state/types";
import { isPluginKind } from "../plugins/kinds";
import { pluginSurface } from "../plugins/registry";
import * as cmd from "../state/commands";
import { TerminalPane } from "../terminal/TerminalPane";
import { terminalContext } from "../terminal/spareShell";
import { AgentPane } from "../agents/AgentPane";
import { DeskHost } from "../workspace/Desk";

export interface WorkbenchItemRendererProps {
    pane: PaneNode;
    session: Session;
    win: WindowT;
    active: boolean;
    visible: boolean;
    /** Whether the screen this pane is on is drawing — on stage, or sliding on or off it. */
    painted: boolean;
}

const EditorPane = lazy(() => import("../editor/EditorPane").then((module) => ({ default: module.EditorPane })));
const GitPane = lazy(() => import("../git/GitPane").then((module) => ({ default: module.GitPane })));
const DiffPane = lazy(() => import("../git/DiffPane").then((module) => ({ default: module.DiffPane })));
const SearchPane = lazy(() => import("../workspace/SearchPane").then((module) => ({ default: module.SearchPane })));
const SharedChatPane = lazy(() => import("../share/SharedChatPane").then((module) => ({ default: module.SharedChatPane })));
const ProjectSettingsPane = lazy(() => import("../settings/ProjectSettingsPane").then((module) => ({ default: module.ProjectSettingsPane })));
const BoardPane = lazy(() => import("../board/BoardPane").then((module) => ({ default: module.BoardPane })));

const paneCwd = (pane: PaneNode, session: Session) => pane.cwd || session.cwd;

function ItemFallback() {
    return <div style={{ width: "100%", height: "100%" }} />;
}

export const BUILTIN_ITEM_RENDERERS: Readonly<Record<CorePaneKind, (props: WorkbenchItemRendererProps) => ReactNode>> = {
    editor: ({ pane, session, win, active, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <EditorPane
                paneId={pane.id}
                cwd={paneCwd(pane, session)}
                host={pane.host}
                active={active}
                visible={visible}
                showInsights={win.role !== "ssh-config"}
                onCloseWindow={win.role === "ssh-config" ? () => cmd.closeSession(session.id) : undefined}
                languageHint={win.role === "ssh-config" ? "ssh-config" : undefined}
            />
        </Suspense>
    ),
    git: ({ pane, session, active, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <GitPane paneId={pane.id} cwd={paneCwd(pane, session)} active={active} visible={visible} />
        </Suspense>
    ),
    diff: ({ pane, session, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <DiffPane cwd={paneCwd(pane, session)} visible={visible} />
        </Suspense>
    ),
    search: ({ pane, session, active, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <SearchPane sessionId={session.id} cwd={paneCwd(pane, session)} active={active} visible={visible} />
        </Suspense>
    ),
    agent: ({ pane, session, visible }) => <AgentPane paneId={pane.id} session={session} visible={visible} />,
    shared: ({ pane, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <SharedChatPane paneId={pane.id} link={pane.startup ?? ""} visible={visible} />
        </Suspense>
    ),
    board: ({ pane, session, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <BoardPane folder={paneCwd(pane, session)} visible={visible} />
        </Suspense>
    ),
    "project-settings": ({ pane, session, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <ProjectSettingsPane project={paneCwd(pane, session)} name={session.name} visible={visible} />
        </Suspense>
    ),
    desk: ({ pane, session, win, active, visible, painted }) => (
        <DeskHost
            paneId={pane.id}
            session={session}
            win={win}
            active={active}
            visible={visible}
            painted={painted}
            onEmpty={() => cmd.removeDeskPane(pane.id)}
        />
    ),
    terminal: ({ pane, session, win, active, visible }) => (
        <TerminalPane
            cwd={paneCwd(pane, session) || undefined}
            startup={pane.startup}
            reconnect={session.kind === "ssh" ? pane.startup : undefined}
            active={active}
            visible={visible}
            context={pane.host ? { ...terminalContext(session, win.id, pane.id), project: pane.cwd } : terminalContext(session, win.id, pane.id)}
            taskTerminalKey={pane.externalPty ? pane.taskTerminalKey : undefined}
            retainPtyOnUnmount
            resumePtyId={pane.ptyId}
            onPtySession={(id) => cmd.setPanePty(pane.id, id)}
            host={pane.host}
        />
    ),
};

export function renderWorkbenchItem(props: WorkbenchItemRendererProps): ReactNode {
    const kind = props.pane.kind;
    if (!isPluginKind(kind)) return BUILTIN_ITEM_RENDERERS[kind](props);
    const surface = pluginSurface(kind);
    if (!surface) return <ItemFallback />;
    return <Suspense fallback={<ItemFallback />}>{surface.render({ paneId: props.pane.id, visible: props.visible })}</Suspense>;
}
