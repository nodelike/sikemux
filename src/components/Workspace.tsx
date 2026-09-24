import { pluginDocuments, usePluginDocumentsVersion } from "../plugins/documents";
import { memo, useMemo, useRef } from "react";
import { useShallow } from "zustand/react/shallow";
import type { CSSProperties, PointerEvent as ReactPointerEvent, RefObject } from "react";
import type { Agent, CorePaneKind, Divider, PaneKind, Rect, Session, TabRef, Window as WindowT, WindowRole } from "../state/types";
import { isPluginKind, type PluginKind } from "../plugins/kinds";
import { pluginSurface } from "../plugins/registry";
import { collectPanes, computeLayout, findSplit, MIN_FRAC } from "../state/layout";
import * as cmd from "../state/commands";
import { getState, useStore } from "../state/store";
import {
    activeTabRef,
    agentPaneId,
    documentsOf,
    expandTabRefs,
    selectSwipeOrder,
    selectTabRefs,
    tabRefKey,
    workspaceTabDropAllowed,
} from "../state/selectors";
import { type CtxItem } from "./FileTree";
import { ErrorBoundary } from "./ErrorBoundary";
import { ShaderField } from "./ShaderField";
import { TabBar, type TabDescriptor } from "./TabBar";
import { AgentIcon, IconPlus, WindowIcon } from "./Icons";
import { AgentStateIndicator, SubagentCount } from "./AgentStateIndicator";
import { renderWorkbenchItem } from "../workbench/renderers";
import { FileIcon } from "./FileIcon";
import { fsapi } from "../api/fs";
import { useStageMotion } from "../state/nativeViews";
import { basename, relativePath } from "../lib/paths";
import { FILE_MANAGER_NAME, PRIMARY_SHORTCUT } from "../lib/platform";
import { notify, reportError } from "../state/toast";
import { copyText } from "../lib/clipboard";
import { PAN_MS, panOffset, useWindowPan } from "./useWindowPan";
import { useWheelPan } from "./useWheelPan";
import { useDocumentSlide } from "./useDocumentSlide";

const copyPath = (_path: string, text: string, label: string) => copyText(text).then(() => notify("success", `copied ${label}`), reportError("copy"));

const FULL: Rect = { x: 0, y: 0, w: 1, h: 1 };

const CORE_PANE_ROLE: Record<CorePaneKind, WindowRole> = {
    terminal: "term",
    editor: "files",
    git: "git",
    diff: "diff",
    search: "search",
    agent: "agent",
    /* A browser is a pane, not a window role of its own. */
    browser: "named",
};

const paneRole = (kind: PaneKind): WindowRole => (isPluginKind(kind) ? kind : CORE_PANE_ROLE[kind]);
const pct = (n: number) => `${n * 100}%`;

/*
 * How many off-screen workbench screens stay mounted.
 *
 * A terminal, editor or git screen is kept alive after you leave it so coming
 * back is instant and a shell is not re-attached. That retention used to be
 * unbounded and only ever released when the window was deleted, so a long
 * session accumulated every screen it had ever shown — each one holding store
 * subscriptions, CodeMirror views and xterm buffers. Eight is more than a
 * working set and small enough to have an end.
 */
const RETAINED_WORKBENCH_WINDOWS = 8;

/**
 * Note that `liveId` is on screen and drop whatever fell out of the working set.
 *
 * Insertion order is the recency order: re-adding an id moves it to the back,
 * so the one dropped is always the one visited longest ago.
 */
export function retainWorkbenchWindows(
    retained: Set<string>,
    liveId: string | null,
    exists: (id: string) => boolean,
    limit = RETAINED_WORKBENCH_WINDOWS,
): Set<string> {
    for (const id of retained) if (!exists(id)) retained.delete(id);
    if (liveId) {
        retained.delete(liveId);
        retained.add(liveId);
    }
    for (const id of retained) {
        if (retained.size <= limit) break;
        retained.delete(id);
    }
    return retained;
}

export const Workspace = memo(function Workspace() {
    const sessionsById = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const windowsById = useStore((s) => s.windows);
    const agentsById = useStore((s) => s.agents);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const activeSessionId = useStore((s) => s.activeSessionId);
    const editorViews = useStore((s) => s.editorViews);
    usePluginDocumentsVersion();
    const areaRef = useRef<HTMLDivElement>(null);
    const mountedWorkbenchWindows = useRef(new Set<string>());

    const sessions = sessionOrder.map((id) => sessionsById[id]);
    const activeSession = sessionsById[activeSessionId];
    const liveWindow = activeSession ? windowsById[activeTabRef(activeSession, windowsById, editorViews)?.id ?? ""] : undefined;
    const liveWorkbenchId =
        liveWindow && (liveWindow.role === "git" || liveWindow.role === "files" || liveWindow.role === "term") ? liveWindow.id : null;
    const retained = retainWorkbenchWindows(mountedWorkbenchWindows.current, liveWorkbenchId, (id) => id in windowsById);
    const activeOrder = useStore(useShallow((state) => selectSwipeOrder(state, state.activeSessionId)));
    const activeSlots = useMemo(() => new Map(activeOrder.map((wid, slot) => [wid, slot])), [activeOrder]);
    const pan = useWindowPan(activeSessionId, activeSession?.activeWindowId ?? null, activeSlots);
    useWheelPan(areaRef, pan);
    // The browser pages are native views placed by measurement, so they only
    // travel with their screen if they know the stage is moving.
    useStageMotion(pan.panning);
    // Counts what the strip would actually show, by asking the list the strip
    // renders: a project holding only rail-driven surfaces has no tabs, and no
    // strip, while an editor or a plugin holding documents counts its open ones.
    const tabCount = useStore((state) => (state.sessions[state.activeSessionId] ? selectTabRefs(state, state.activeSessionId).length : 0));

    // The strip is what the screens start below, so its absence is what the
    // stage has to know about: with no tabs there is nothing to start below.
    const strip = activeSession && tabCount > 0 ? <WorkspaceTabsBar session={activeSession} /> : null;

    return (
        <div className={`window-area${strip ? " window-area--strip" : ""}`} ref={areaRef}>
            {strip}
            {sessions.map((session) => {
                const isActive = session.id === activeSessionId;
                const active = activeTabRef(session, windowsById, editorViews);
                const activeWindowId = active?.id ?? null;
                const order = windowsBySession[session.id] ?? EMPTY_IDS;
                return (
                    <div
                        key={session.id}
                        ref={isActive ? pan.trackRef : undefined}
                        className={`window-track${isActive && pan.panning ? " panning" : ""}${isActive && pan.sliding ? " sliding" : ""}${
                            isActive && pan.returning ? " returning" : ""
                        }`}
                        style={
                            {
                                "--window-pan-ms": `${isActive ? pan.ms : PAN_MS}ms`,
                                "--pan": panOffset(isActive ? pan.at : order.indexOf(session.activeWindowId)),
                            } as CSSProperties
                        }>
                        {order.map((wid, slot) => {
                            const win = windowsById[wid];
                            if (!win) return null;
                            const live = isActive && activeWindowId === wid;
                            const painted = isActive && pan.paints(wid);
                            // A live agent keeps its process whether or not it is on screen;
                            // a sleeping one has nothing to keep.
                            const keepsProcess =
                                win.role === "agent" ? agentsById[agentPaneId(win) ?? ""]?.launchState !== "dormant" : retained.has(wid);
                            // A layer sliding out has to stay mounted for as long as it paints.
                            if (!live && !painted && wid !== session.activeWindowId && !keepsProcess) return null;
                            return (
                                <WindowLayer
                                    key={wid}
                                    session={session}
                                    win={win}
                                    areaRef={areaRef}
                                    slot={isActive ? pan.slotOf(wid, activeSlots.get(wid) ?? slot) : slot}
                                    live={live}
                                    painted={painted}
                                />
                            );
                        })}
                    </div>
                );
            })}
            {activeSession && activeOrder.length > 1 && activeSlots.has(activeSession.activeWindowId) && (
                <WindowScrollIndicator count={activeOrder.length} index={activeOrder.indexOf(activeSession.activeWindowId)} ms={pan.ms} />
            )}
        </div>
    );
});

/**
 * Where the session sits along its screens, as a thumb the width of one screen.
 * It reads the window's real place in the session rather than the screen a slide
 * has it parked on, so a jump of several screens travels the whole way here
 * while the canvas next door slides one.
 */
function WindowScrollIndicator({ count, index, ms }: { count: number; index: number; ms: number }) {
    return (
        <div className="window-scroll" aria-hidden="true" style={{ "--window-pan-ms": `${ms}ms` } as CSSProperties}>
            <div className="window-scroll-thumb" style={{ width: `${100 / count}%`, transform: `translateX(${Math.max(0, index) * 100}%)` }} />
        </div>
    );
}

const EMPTY_IDS: readonly string[] = [];

const CORE_ROLE_LABEL: Record<Exclude<WindowRole, PluginKind>, string> = {
    term: "Terminal",
    files: "Editor",
    git: "Git",
    diff: "Diff",
    search: "Search",
    "ssh-config": "SSH config",
    named: "Window",
    agent: "Agent",
};

const roleLabel = (role: WindowRole): string => (isPluginKind(role) ? (pluginSurface(role)?.title ?? role) : CORE_ROLE_LABEL[role]);

/** A workspace tab, already carrying the ids the strip and the live layer pair up with. */
type WorkspaceTab = TabDescriptor & { tabId: string; panelId: string };

const WorkspaceTabsBar = memo(function WorkspaceTabsBar({ session }: { session: Session }) {
    const windowsById = useStore((s) => s.windows);
    const agentsById = useStore((s) => s.agents);
    const activity = useStore((s) => s.agentActivity);
    const backgroundWork = useStore((s) => s.agentBackgroundWork);
    const subagentCounts = useStore((s) => s.agentSubagents);
    const windowIds = useStore((s) => s.windowsBySession[session.id]);
    const editorViews = useStore((s) => s.editorViews);
    const dirtyEditorPaths = useStore((s) => s.dirtyEditorPaths);
    const documentsVersion = usePluginDocumentsVersion();
    // Shared with cycleTab through selectTabRefs, so the strip and the keyboard
    // can never disagree about what the tabs are.
    const refs = useMemo(
        () => expandTabRefs(windowIds ?? EMPTY_IDS, windowsById, editorViews),
        // eslint-disable-next-line react-hooks/exhaustive-deps -- a plugin's documents live outside the store
        [windowIds, windowsById, editorViews, documentsVersion],
    );
    const active = activeTabRef(session, windowsById, editorViews);
    const activeKey = active ? tabRefKey(active) : null;

    /*
     * A terminal tab shows the shell's own title, so only the terminals in this
     * strip are subscribed to rather than the whole record — one `cd` used to
     * re-render every strip and every layer in the app.
     */
    const termPaneIds = useMemo(
        () => refs.flatMap((ref) => (ref.doc === undefined && windowsById[ref.id]?.role === "term" ? [windowsById[ref.id]!.activePaneId] : [])),
        [refs, windowsById],
    );
    const termTitleList = useStore(useShallow((s) => termPaneIds.map((id) => s.terminalTitles[id] ?? "")));
    const termTitles = useMemo(() => new Map(termPaneIds.map((id, index) => [id, termTitleList[index]])), [termPaneIds, termTitleList]);

    const windowMenu = (win: WindowT): CtxItem[] => {
        const siblings = refs.flatMap((ref) => (ref.doc === undefined ? [windowsById[ref.id]] : [])).filter(Boolean) as WindowT[];
        const others = siblings.filter((t) => t.id !== win.id && !t.fixed && t.role !== "agent");
        return [
            { label: "Duplicate", run: () => cmd.duplicateWindow(win.id) },
            { label: "Close", hint: "⌥W", disabled: win.fixed, run: () => cmd.closeWindowById(win.id) },
            { label: "Close Others", disabled: others.length === 0, run: () => others.forEach((t) => cmd.closeWindowById(t.id)) },
        ];
    };

    const fileMenu = (win: WindowT, doc: string): CtxItem[] => {
        const open = editorViews[win.activePaneId]?.openTabs ?? [];
        const dirty = new Set(dirtyEditorPaths[win.activePaneId] ?? []);
        const index = open.indexOf(doc);
        const close = (paths: string[]) => paths.forEach((path) => cmd.closeTab({ id: win.id, doc: path }));
        const others = open.filter((path) => path !== doc);
        const toLeft = index > 0 ? open.slice(0, index) : [];
        const toRight = index >= 0 ? open.slice(index + 1) : [];
        const saved = open.filter((path) => !dirty.has(path));
        return [
            { label: "Close", hint: `${PRIMARY_SHORTCUT}W`, run: () => close([doc]) },
            { label: "Close Others", disabled: others.length === 0, run: () => close(others) },
            { label: "Close to the Left", disabled: toLeft.length === 0, run: () => close(toLeft) },
            { label: "Close to the Right", disabled: toRight.length === 0, run: () => close(toRight) },
            { label: "Close Saved", disabled: saved.length === 0, run: () => close(saved) },
            { label: "Close All", run: () => close(open) },
            { sep: true },
            { label: "Copy Path", run: () => void copyPath(doc, doc, "path") },
            {
                label: "Copy Relative Path",
                run: () => void copyPath(doc, relativePath(doc, session.cwd) ?? basename(doc), "relative path"),
            },
            { sep: true },
            { label: `Reveal in ${FILE_MANAGER_NAME}`, run: () => void fsapi.revealInFinder(doc).catch(reportError("reveal")) },
        ];
    };

    const agentMenu = (agent: Agent): CtxItem[] => {
        const agents = refs
            .flatMap((ref) => {
                const win = ref.doc === undefined ? windowsById[ref.id] : undefined;
                return win?.role === "agent" ? [agentsById[agentPaneId(win) ?? ""]] : [];
            })
            .filter(Boolean) as Agent[];
        const others = agents.filter((x) => x.id !== agent.id);
        const items: CtxItem[] = [
            ...(agent.launchState === "dormant"
                ? [{ label: "Resume", run: () => cmd.selectAgent(agent.id) }]
                : agent.resumeId
                  ? [{ label: "Sleep", run: () => cmd.sleepAgent(agent.id) }]
                  : []),
            ...(agent.resumeId && agent.launchState !== "dormant"
                ? [{ label: agent.keepAlive ? "Allow Auto-Sleep" : "Keep Alive", run: () => cmd.setAgentKeepAlive(agent.id, !agent.keepAlive) }]
                : []),
            ...(agent.resumeId ? [{ sep: true as const }] : []),
            { label: "Close", hint: "⌥W", run: () => cmd.closeAgent(agent.id) },
            { label: "Close Others", disabled: others.length === 0, run: () => others.forEach((x) => cmd.closeAgent(x.id)) },
        ];
        if (cmd.agentSupportsSkipPermissions(agent.type)) {
            const skip = agent.permissionMode === "bypass" || agent.skipPermissions === true;
            items.push(
                { sep: true },
                { label: skip ? "Disable YOLO Mode" : "Enable YOLO Mode", hint: "⌥Y", run: () => cmd.toggleAgentSkipPermissions(agent.id) },
            );
        }
        return items;
    };

    const tabs = useMemo<WorkspaceTab[]>(() => {
        const build = (ref: TabRef): TabDescriptor[] => {
            const key = tabRefKey(ref);
            const win = windowsById[ref.id];
            if (!win) return [];
            const pluginDocs = ref.doc !== undefined ? pluginDocuments(win.role) : undefined;
            if (ref.doc !== undefined && pluginDocs) {
                const tab = pluginDocs.describe(win.activePaneId, ref.doc);
                return [{ id: key, label: tab.label, title: tab.title ?? tab.label, active: key === activeKey, dirty: tab.dirty, icon: tab.icon }];
            }
            if (ref.doc !== undefined) {
                const name = basename(ref.doc);
                return [
                    {
                        id: key,
                        label: name,
                        title: ref.doc,
                        active: key === activeKey,
                        dirty: (dirtyEditorPaths[win.activePaneId] ?? []).includes(ref.doc),
                        icon: <FileIcon name={name} size={16} />,
                    },
                ];
            }
            if (win.role === "agent") {
                const agent = agentsById[agentPaneId(win) ?? ""];
                if (!agent) return [];
                const state = activity[agent.id];
                const background = (backgroundWork[agent.id] ?? 0) > 0;
                const subagents = subagentCounts[agent.id] ?? 0;
                return [
                    {
                        id: key,
                        label: agent.title,
                        title: agent.title,
                        active: key === activeKey,
                        icon: (
                            <span className={`agent-glyph ${agent.type}`}>
                                <AgentIcon type={agent.type} size={19} />
                            </span>
                        ),
                        badge: subagents > 0 ? <SubagentCount count={subagents} /> : undefined,
                        accessory: state || background ? <AgentStateIndicator state={state?.state ?? "idle"} background={background} /> : undefined,
                    },
                ];
            }
            const label = win.role === "term" ? termTitles.get(win.activePaneId) || win.name : roleLabel(win.role);
            return [
                {
                    id: key,
                    label,
                    title: label,
                    active: key === activeKey,
                    closable: !win.fixed,
                    icon: (
                        <span className="agent-glyph">
                            <WindowIcon role={win.role} size={13} />
                        </span>
                    ),
                },
            ];
        };
        return refs.flatMap((ref) =>
            build(ref).map((tab) => ({
                ...tab,
                tabId: `workspace-tab-${session.id}-${encodeURIComponent(tab.id)}`,
                panelId: `workspace-content-${session.id}`,
            })),
        );
        // eslint-disable-next-line react-hooks/exhaustive-deps -- a plugin's documents live outside the store
    }, [
        refs,
        windowsById,
        agentsById,
        activity,
        backgroundWork,
        subagentCounts,
        termTitles,
        dirtyEditorPaths,
        activeKey,
        session.id,
        documentsVersion,
    ]);

    const refByKey = new Map(refs.map((ref) => [tabRefKey(ref), ref]));

    return (
        <TabBar
            variant="agent"
            tabs={tabs}
            onSelect={(key) => {
                const ref = refByKey.get(key);
                if (ref) cmd.selectTab(ref);
            }}
            onClose={(key) => {
                const ref = refByKey.get(key);
                if (ref) cmd.closeTab(ref);
            }}
            buildMenu={(key) => {
                const ref = refByKey.get(key);
                if (!ref) return [];
                const win = windowsById[ref.id];
                if (!win) return [];
                const pluginDocs = ref.doc !== undefined ? pluginDocuments(win.role) : undefined;
                if (ref.doc !== undefined && pluginDocs) {
                    const doc = ref.doc;
                    return pluginDocs.menu ? [...pluginDocs.menu(win.activePaneId, doc)] : [{ label: "Close", run: () => cmd.closeTab(ref) }];
                }
                if (ref.doc !== undefined) return fileMenu(win, ref.doc);
                if (win.role === "agent") {
                    const agent = agentsById[agentPaneId(win) ?? ""];
                    return agent ? agentMenu(agent) : [];
                }
                return windowMenu(win);
            }}
            onAdd={() => cmd.openNewTabPalette()}
            addIcon={<IconPlus size={13} />}
            addTitle="New tab"
            canReorder={(sourceKey, targetKey, placement) => {
                const source = refByKey.get(sourceKey);
                const target = refByKey.get(targetKey);
                return !!source && !!target && workspaceTabDropAllowed(refs, source, target, placement);
            }}
            onReorder={(sourceKey, targetKey, placement) => {
                const source = refByKey.get(sourceKey);
                const target = refByKey.get(targetKey);
                if (!source || !target) return;
                // Beside another window's documents means beside that window.
                if (source.doc !== undefined && target.doc !== undefined) cmd.reorderDocumentTab(source.id, source.doc, target.doc, placement);
                else cmd.reorderWindowTab(session.id, source.id, target.id, placement);
            }}
        />
    );
});

const WindowLayer = memo(function WindowLayer({
    session,
    win,
    live,
    painted,
    slot,
    areaRef,
}: {
    session: Session;
    win: WindowT;
    /** Whether this window is the one the session is on: panes spawn, hydrate and poll on it. */
    live: boolean;
    /** Whether the layer paints at all. A painted layer that is not live shows what it already has. */
    painted: boolean;
    /** Which screen along the track this layer sits on, counted from the track's left edge. */
    slot: number;
    areaRef: RefObject<HTMLDivElement | null>;
}) {
    const editorView = useStore((s) => s.editorViews[win.activePaneId]);
    usePluginDocumentsVersion();
    const editorViews = editorView ? { [win.activePaneId]: editorView } : {};
    const active = activeTabRef(session, { [win.id]: win }, editorViews);
    const documents = documentsOf(win, editorViews);
    const layerRef = useRef<HTMLDivElement>(null);
    useDocumentSlide(layerRef, live ? win.activePaneId : null, documents?.activeId ?? null, documents?.ids ?? EMPTY_IDS);
    const zoomedPaneId = useStore((s) => s.zoomedPaneId);
    const { panes, dividers, stacked, stacks, inStack } = useMemo(() => computeLayout(win.root, win.activePaneId), [win.root, win.activePaneId]);
    /*
     * Only the panes stacked behind a strip show a shell's own title, so only
     * those are subscribed to. Reading the whole title record here meant one
     * prompt or `cd` anywhere re-rendered every retained layer in the app, and
     * every pane inside them.
     */
    const stackPaneIds = useMemo(() => stacks.flatMap((stack) => stack.tabs.map((pane) => pane.id)), [stacks]);
    const stackTitleList = useStore(useShallow((s) => stackPaneIds.map((id) => s.terminalTitles[id] ?? "")));
    const stackTitles = useMemo(() => new Map(stackPaneIds.map((id, index) => [id, stackTitleList[index]])), [stackPaneIds, stackTitleList]);
    const leaves = useMemo(() => collectPanes(win.root), [win.root]);
    const zoomActive = live && zoomedPaneId != null;

    return (
        <div
            ref={layerRef}
            className={`window-layer${live ? " live" : ""}${painted ? " painted" : ""}`}
            id={live ? `workspace-content-${session.id}` : undefined}
            role="tabpanel"
            aria-labelledby={active ? `workspace-tab-${session.id}-${encodeURIComponent(tabRefKey(active))}` : undefined}
            aria-hidden={!live}
            inert={!live}
            style={{ "--slot": slot } as CSSProperties}>
            {leaves.map((p) => {
                const isZoomed = zoomedPaneId === p.id;
                // A pane a stack is covering keeps its cell, and its size, so it
                // does not have to re-measure when it comes back to the top.
                const behind = !panes.has(p.id);
                const shown = (!zoomActive || isZoomed) && !behind;
                const rect = isZoomed ? FULL : (panes.get(p.id) ?? stacked.get(p.id))!;
                const isActive = p.id === win.activePaneId;
                const paneVisible = live && shown;
                const paneActive = paneVisible && isActive;
                const panePainted = painted && shown;
                return (
                    <div
                        key={p.id}
                        className={`pane-cell${inStack.has(p.id) ? " in-stack" : ""}`}
                        style={{
                            left: pct(rect.x),
                            top: pct(rect.y),
                            width: pct(rect.w),
                            height: pct(rect.h),
                            visibility: shown ? undefined : "hidden",
                            zIndex: isZoomed ? 2 : 1,
                        }}>
                        <div
                            className={`pane pane-${isPluginKind(p.kind) ? "plugin" : p.kind}`}
                            data-pane-id={p.id}
                            onMouseDown={() => live && cmd.focusPane(p.id)}>
                            {/* The pane is a surface, so it carries its own texture — and only
                                while it is the one being read, so a screen off stage spends no
                                WebGL context on a field nobody is looking at. */}
                            <ShaderField preset="ambient" className="pane-field" enabled={live && shown} />
                            <ErrorBoundary label={`${p.kind} pane`}>
                                {renderWorkbenchItem({ pane: p, session, win, active: paneActive, visible: paneVisible, painted: panePainted })}
                            </ErrorBoundary>
                        </div>
                    </div>
                );
            })}
            {!zoomActive &&
                stacks.map((stack) => (
                    <div
                        key={stack.splitId}
                        className="stack-strip"
                        style={{ left: pct(stack.rect.x), top: pct(stack.rect.y), width: pct(stack.rect.w) }}>
                        <TabBar
                            variant="stack"
                            ariaLabel="Panes in this stack"
                            tabs={stack.tabs.map((pane) => ({
                                id: pane.id,
                                label: stackTitles.get(pane.id) || pane.title,
                                title: stackTitles.get(pane.id) || pane.title,
                                active: pane.id === stack.activePaneId,
                                icon: (
                                    <span className="agent-glyph">
                                        <WindowIcon role={paneRole(pane.kind)} size={12} />
                                    </span>
                                ),
                                closable: false,
                            }))}
                            onSelect={(paneId) => live && cmd.focusPane(paneId)}
                        />
                    </div>
                ))}
            {live && !zoomActive && dividers.map((d) => <DividerHandle key={`${d.splitId}:${d.index}`} d={d} windowId={win.id} areaRef={areaRef} />)}
        </div>
    );
});

function DividerHandle({ d, windowId, areaRef }: { d: Divider; windowId: string; areaRef: RefObject<HTMLDivElement | null> }) {
    const horizontal = d.dir === "row";

    const style = horizontal
        ? {
              left: pct(d.rect.x + d.at * d.rect.w),
              top: pct(d.rect.y),
              height: pct(d.rect.h),
          }
        : {
              top: pct(d.rect.y + d.at * d.rect.h),
              left: pct(d.rect.x),
              width: pct(d.rect.w),
          };

    const onPointerDown = (e: ReactPointerEvent) => {
        e.preventDefault();
        const handle = e.currentTarget as HTMLDivElement;
        const area = areaRef.current;
        if (!area) return;
        const bounds = area.getBoundingClientRect();
        const st = getState();
        const winNode = st.windows[windowId];
        const split = winNode ? findSplit(winNode.root, d.splitId) : null;
        if (!split) return;
        handle.setPointerCapture(e.pointerId);

        const startSizes = split.sizes.slice();
        const i = d.index;
        const axisPx = horizontal ? bounds.width * d.rect.w : bounds.height * d.rect.h;
        const start = horizontal ? e.clientX : e.clientY;

        let frame: number | null = null;
        let pendingSizes: number[] | null = null;
        const commitPending = () => {
            frame = null;
            if (!pendingSizes) return;
            cmd.setSplitSizes(windowId, d.splitId, pendingSizes);
            pendingSizes = null;
        };
        const move = (ev: PointerEvent) => {
            let df = ((horizontal ? ev.clientX : ev.clientY) - start) / axisPx;
            df = Math.max(-(startSizes[i] - MIN_FRAC), Math.min(startSizes[i + 1] - MIN_FRAC, df));
            const sizes = startSizes.slice();
            sizes[i] += df;
            sizes[i + 1] -= df;
            pendingSizes = sizes;
            if (frame == null) frame = window.requestAnimationFrame(commitPending);
        };
        const up = () => {
            if (frame != null) window.cancelAnimationFrame(frame);
            commitPending();
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", up);
            handle.removeEventListener("pointercancel", up);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
        handle.addEventListener("pointercancel", up);
    };

    const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
        const direction = horizontal
            ? e.key === "ArrowLeft"
                ? -1
                : e.key === "ArrowRight"
                  ? 1
                  : 0
            : e.key === "ArrowUp"
              ? -1
              : e.key === "ArrowDown"
                ? 1
                : 0;
        if (!direction) return;
        e.preventDefault();
        const winNode = getState().windows[windowId];
        const split = winNode ? findSplit(winNode.root, d.splitId) : null;
        if (!split) return;
        const sizes = split.sizes.slice();
        const step = e.shiftKey ? 0.05 : 0.02;
        const delta = Math.max(-(sizes[d.index] - MIN_FRAC), Math.min(sizes[d.index + 1] - MIN_FRAC, direction * step));
        sizes[d.index] += delta;
        sizes[d.index + 1] -= delta;
        cmd.setSplitSizes(windowId, d.splitId, sizes);
    };

    return (
        <div
            className={`divider divider-${d.dir}`}
            style={style}
            role="separator"
            tabIndex={0}
            aria-orientation={horizontal ? "vertical" : "horizontal"}
            title="Drag or use arrow keys to resize"
            onPointerDown={onPointerDown}
            onKeyDown={onKeyDown}
        />
    );
}
