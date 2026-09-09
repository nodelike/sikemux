import { lazy, Suspense, type ReactNode } from "react";
import type { PaneKind, PaneNode, PtyContext, Session, Window as WindowT } from "../state/types";
import * as cmd from "../state/commands";
import { TerminalPane } from "../terminal/TerminalPane";

export interface WorkbenchItemRendererProps {
    pane: PaneNode;
    session: Session;
    win: WindowT;
    active: boolean;
    visible: boolean;
}

const EditorPane = lazy(() => import("../components/EditorPane").then((module) => ({ default: module.EditorPane })));
const GitPane = lazy(() => import("../components/GitPane").then((module) => ({ default: module.GitPane })));
const DiffPane = lazy(() => import("../components/DiffPane").then((module) => ({ default: module.DiffPane })));
const AwsPane = lazy(() => import("../components/aws/AwsPane").then((module) => ({ default: module.AwsPane })));
const RundeckPane = lazy(() => import("../components/rundeck/RundeckPane").then((module) => ({ default: module.RundeckPane })));
const BrunoPane = lazy(() => import("../components/bruno/BrunoPane").then((module) => ({ default: module.BrunoPane })));
const SearchPane = lazy(() => import("../components/SearchPane").then((module) => ({ default: module.SearchPane })));

const paneCwd = (pane: PaneNode, session: Session) => pane.cwd || session.cwd;
const terminalContext = (session: Session, win: WindowT, pane: PaneNode): PtyContext => ({
    sessionId: session.id,
    sessionName: session.name,
    sessionKind: session.kind,
    ...(session.kind === "project" && session.cwd ? { project: session.cwd } : {}),
    windowId: win.id,
    paneId: pane.id,
    shellIntegration: session.kind === "project" || session.kind === "command",
});

function ItemFallback() {
    return <div style={{ width: "100%", height: "100%" }} />;
}

export const BUILTIN_ITEM_RENDERERS: Readonly<Record<PaneKind, (props: WorkbenchItemRendererProps) => ReactNode>> = {
    editor: ({ pane, session, win, active, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <EditorPane
                paneId={pane.id}
                cwd={paneCwd(pane, session)}
                active={active}
                visible={visible}
                showInsights={win.role !== "ssh-config"}
                onCloseWindow={win.role === "ssh-config" ? () => cmd.closeSession(session.id) : undefined}
                languageHint={win.role === "ssh-config" ? "ssh-config" : undefined}
            />
        </Suspense>
    ),
    git: ({ pane, session, active }) => (
        <Suspense fallback={<ItemFallback />}>
            <GitPane paneId={pane.id} cwd={paneCwd(pane, session)} active={active} />
        </Suspense>
    ),
    diff: ({ pane, session, active }) => (
        <Suspense fallback={<ItemFallback />}>
            <DiffPane cwd={paneCwd(pane, session)} active={active} />
        </Suspense>
    ),
    aws: ({ visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <AwsPane active={visible} />
        </Suspense>
    ),
    rundeck: ({ pane, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <RundeckPane paneId={pane.id} active={visible} />
        </Suspense>
    ),
    bruno: ({ pane, session, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <BrunoPane paneId={pane.id} sessionId={session.id} active={visible} />
        </Suspense>
    ),
    search: ({ pane, session, active, visible }) => (
        <Suspense fallback={<ItemFallback />}>
            <SearchPane sessionId={session.id} cwd={paneCwd(pane, session)} active={active} visible={visible} />
        </Suspense>
    ),
    terminal: ({ pane, session, win, active, visible }) => (
        <TerminalPane
            cwd={paneCwd(pane, session) || undefined}
            startup={pane.startup}
            active={active}
            visible={visible}
            context={terminalContext(session, win, pane)}
            externallyOwned={pane.externalPty === true}
            retainPtyOnUnmount
            onTitleChange={(title) => cmd.setTerminalTitle(pane.id, title)}
        />
    ),
};

export function renderWorkbenchItem(props: WorkbenchItemRendererProps): ReactNode {
    return BUILTIN_ITEM_RENDERERS[props.pane.kind](props);
}
