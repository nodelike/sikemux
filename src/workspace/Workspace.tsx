import { pluginDocuments, usePluginDocumentsVersion } from "../plugins/documents";
import { memo, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import type { CSSProperties, PointerEvent as ReactPointerEvent, RefObject } from "react";
import type { Agent, Divider, PaneNode, Rect, Session, TabRef, Window as WindowT } from "../state/types";
import { isPluginKind } from "../plugins/kinds";
import { collectPanes, computeLayout, findSplit, MIN_FRAC, openSides } from "../state/layout";
import { cellPlacement, dividerKey, dividerPlacement, stackPlacement } from "../state/layoutPaint";
import * as cmd from "../state/commands";
import { getState, useStore } from "../state/store";
import {
    activeTabRef,
    agentDisplayOf,
    agentInWindow,
    documentsOf,
    editorPaneOf,
    expandTabRefs,
    selectSwipeOrder,
    selectTabRefs,
    paneToSeparate,
    tabRefKey,
    tabSplitAllowed,
    terminalLabel,
    workspaceTabDropAllowed,
    type SplitSide,
} from "../state/selectors";
import { type CtxItem } from "../rail/FileTree";
import { ErrorBoundary } from "../ui/ErrorBoundary";
import { PaneField, PanePaintedContext, ScreenFieldContext } from "../ui/ShaderField";
import { agentMenu, renameAgentNamePrompt, renameAgentPrompt } from "./agentMenu";
import { TabBar, type TabDescriptor } from "./TabBar";
import type { TabDragOut, TabPoint } from "./useTabReorder";
import { AgentFace } from "../buddies/Buddy";
import { IconArrowUp, IconClose, IconMove, IconPlus, WindowIcon } from "../ui/Icons";
import { agentHandle } from "../agents/AgentLabel";
import { AgentStateIndicator, BackgroundCounts, showsAgentState } from "../agents/AgentStateIndicator";
import { renderWorkbenchItem } from "../workbench/renderers";
import { paneRole, windowRoleLabel } from "../state/windowRoles";
import { FileIcon } from "../ui/FileIcon";
import { useStageMotion } from "../state/nativeViews";
import { basename } from "../lib/paths";
import { useShortcutLabel } from "../commands/useShortcutLabel";
import { PAN_MS, panTransform, useWindowPan } from "./useWindowPan";
import { useWheelPan } from "./useWheelPan";
import { useDocumentSlide } from "./useDocumentSlide";
import { fileTabMenu, renameTerminalTab, windowTabMenu } from "./tabMenus";

const FULL: Rect = { x: 0, y: 0, w: 1, h: 1 };

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

    return (
        <div className="window-area" ref={areaRef}>
            {sessions.map((session) => {
                const isActive = session.id === activeSessionId;
                const active = activeTabRef(session, windowsById, editorViews);
                const activeWindowId = active?.id ?? null;
                const order = windowsBySession[session.id] ?? EMPTY_IDS;
                return (
                    <div
                        key={session.id}
                        ref={isActive ? pan.trackRef : undefined}
                        className={`window-track${isActive && pan.panning ? " panning" : ""}${isActive && pan.sliding ? " sliding" : ""}`}
                        style={
                            {
                                "--window-pan-ms": `${isActive ? pan.ms : PAN_MS}ms`,
                                transform: isActive && pan.panning ? panTransform(pan.at) : undefined,
                            } as CSSProperties
                        }>
                        {order.map((wid, slot) => {
                            const win = windowsById[wid];
                            if (!win) return null;
                            const live = isActive && activeWindowId === wid;
                            const painted = isActive && pan.paints(wid);
                            // A live agent keeps its process whether or not it is on screen;
                            // a sleeping one has nothing to keep.
                            const keepsProcess = win.role === "agent" ? agentInWindow(agentsById, win)?.launchState !== "dormant" : retained.has(wid);
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
            {activeSession?.kind === "project" && (windowsBySession[activeSession.id] ?? EMPTY_IDS).length === 0 && <EmptyStage />}
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

/** A workspace tab, already carrying the ids the strip and the live layer pair up with. */
type WorkspaceTab = TabDescriptor & { tabId: string; panelId: string };

interface SplitTarget {
    paneId: string;
    side: SplitSide;
    /** The half of the pane the dropped tab will take, in window coordinates. */
    box: { left: number; top: number; width: number; height: number };
}

/** The pane of the tab on screen that is under `point`, and the edge of it the point is nearest that can still take a pane. */
function splitTargetAt(sessionId: string, point: TabPoint): SplitTarget | null {
    const state = getState();
    const shown = state.windows[state.sessions[sessionId]?.activeWindowId ?? ""];
    if (!shown) return null;
    const open = openSides(shown.root);
    const own = new Set(collectPanes(shown.root).map((pane) => pane.id));
    const cell = document
        .elementsFromPoint(point.x, point.y)
        .map((element) => element.closest<HTMLElement>("[data-pane-id]"))
        .find((element) => own.has(element?.dataset.paneId ?? ""));
    if (!cell || open.length === 0) return null;
    const pane = cell.getBoundingClientRect();
    const distance: Record<SplitSide, number> = {
        left: (point.x - pane.left) / pane.width,
        right: (pane.right - point.x) / pane.width,
        top: (point.y - pane.top) / pane.height,
        bottom: (pane.bottom - point.y) / pane.height,
    };
    const side = open.reduce((nearest, edge) => (distance[edge] < distance[nearest] ? edge : nearest));
    const across = side === "left" || side === "right";
    return {
        paneId: cell.dataset.paneId!,
        side,
        box: {
            left: pane.left + (side === "right" ? pane.width / 2 : 0),
            top: pane.top + (side === "bottom" ? pane.height / 2 : 0),
            width: across ? pane.width / 2 : pane.width,
            height: across ? pane.height : pane.height / 2,
        },
    };
}

/** A project with every tab closed, which would otherwise show nothing to click. */
function EmptyStage() {
    const moves = [
        { label: "New agent", shortcut: useShortcutLabel("agent.new"), run: () => void cmd.startAgent() },
        { label: "New terminal", shortcut: useShortcutLabel("terminal.new"), run: cmd.newTerminal },
        { label: "Open file", shortcut: useShortcutLabel("palette.files"), run: cmd.openFilePalette },
    ];
    return (
        <div className="empty-stage">
            <span className="empty-stage-title">Nothing open in this project</span>
            <div className="empty-stage-moves">
                {moves.map((move) => (
                    <button key={move.label} type="button" className="empty-stage-move" onClick={move.run}>
                        <span>{move.label}</span>
                        {move.shortcut && <kbd>{move.shortcut}</kbd>}
                    </button>
                ))}
            </div>
        </div>
    );
}

/** The active session's tabs. A project holding only rail-driven surfaces has none, and shows no strip. */
export function WorkspaceTabs() {
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const tabCount = useStore((state) => (state.sessions[state.activeSessionId] ? selectTabRefs(state, state.activeSessionId).length : 0));
    return session && tabCount > 0 ? <WorkspaceTabsBar session={session} /> : null;
}

const WorkspaceTabsBar = memo(function WorkspaceTabsBar({ session }: { session: Session }) {
    const [splitTarget, setSplitTarget] = useState<SplitTarget | null>(null);
    const windowsById = useStore((s) => s.windows);
    const agentsById = useStore((s) => s.agents);
    const agentStatus = useStore((s) => s.agentStatus);
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
    const termPtyIds = useMemo(
        () =>
            (windowIds ?? EMPTY_IDS).flatMap((id) => {
                const win = windowsById[id];
                return win
                    ? collectPanes(win.root).flatMap((pane) => (pane.kind === "terminal" && pane.ptyId !== undefined ? [String(pane.ptyId)] : []))
                    : [];
            }),
        [windowIds, windowsById],
    );
    const termTitleList = useStore(useShallow((s) => termPtyIds.map((id) => s.terminalTitles[id] ?? "")));
    const termTitles = useMemo(() => Object.fromEntries(termPtyIds.map((id, index) => [id, termTitleList[index]])), [termPtyIds, termTitleList]);

    const closeShortcut = useShortcutLabel("pane.close");
    const permissionsShortcut = useShortcutLabel("agent.permissions");

    const agentTabMenu = (agent: Agent): CtxItem[] => {
        const others = refs
            .flatMap((ref) => {
                const win = ref.doc === undefined ? windowsById[ref.id] : undefined;
                return win?.role === "agent" ? [agentInWindow(agentsById, win)] : [];
            })
            .filter((x): x is Agent => !!x && x.id !== agent.id);
        return agentMenu(agent, others, session, { close: closeShortcut, permissions: permissionsShortcut });
    };

    const { tabs, paneOfTab } = useMemo(() => {
        const paneOfTab = new Map<string, { windowId: string; paneId: string }>();
        const look = (pane: PaneNode, win: WindowT): Pick<TabDescriptor, "label" | "title" | "icon" | "note"> => {
            if (pane.kind === "terminal") {
                const label = terminalLabel(win, pane, termTitles);
                return {
                    label,
                    title: label,
                    icon: (
                        <span className="agent-glyph term">
                            <WindowIcon role="term" size={13} />
                        </span>
                    ),
                };
            }
            if (pane.kind === "agent") {
                const agent = agentsById[pane.id];
                const title = agent?.title ?? pane.title;
                return {
                    label: title,
                    title,
                    note: agent?.name && agentHandle(agent.name),
                    icon: agent ? (
                        <AgentFace
                            agent={agent}
                            size={20}
                            logoSize={19}
                            logoClass={`agent-glyph ${agent.type}`}
                            buddyClass="agent-glyph buddy-mark"
                        />
                    ) : undefined,
                };
            }
            const path = pane.kind === "editor" ? editorViews[pane.id]?.activePath : null;
            if (path) return { label: basename(path), title: path, icon: <FileIcon name={basename(path)} size={14} /> };
            const label = windowRoleLabel(paneRole(pane.kind));
            return {
                label,
                title: label,
                icon: (
                    <span className={`agent-glyph ${paneRole(pane.kind)}`}>
                        <WindowIcon role={paneRole(pane.kind)} size={13} />
                    </span>
                ),
            };
        };
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
                        dirty: (dirtyEditorPaths[editorPaneOf(win, editorViews)] ?? []).includes(ref.doc),
                        preview: editorViews[editorPaneOf(win, editorViews)]?.preview === ref.doc,
                        icon: <FileIcon name={name} size={14} />,
                    },
                ];
            }
            if (win.role === "agent") {
                const agent = agentInWindow(agentsById, win);
                if (!agent) return [];
                const display = agentDisplayOf({ agentStatus }, agent.id);
                return [
                    {
                        id: key,
                        label: agent.title,
                        title: agent.title,
                        active: key === activeKey,
                        note: agent.name && agentHandle(agent.name),
                        icon: (
                            <AgentFace
                                agent={agent}
                                size={20}
                                logoSize={19}
                                logoClass={`agent-glyph ${agent.type}`}
                                buddyClass="agent-glyph buddy-mark"
                            />
                        ),
                        badge: display.subagents + display.tasks > 0 ? <BackgroundCounts display={display} /> : undefined,
                        accessory: showsAgentState(display) ? <AgentStateIndicator display={display} /> : undefined,
                    },
                ];
            }
            const shown = collectPanes(win.root).find((pane) => pane.id === win.activePaneId);
            const role = shown?.kind === "terminal" ? "term" : win.role;
            const label = role === "term" && shown ? terminalLabel(win, shown, termTitles) : windowRoleLabel(role);
            return [
                {
                    id: key,
                    label,
                    title: label,
                    active: key === activeKey,
                    closable: !win.fixed,
                    icon: (
                        <span className={`agent-glyph ${role}`}>
                            <WindowIcon role={role} size={13} />
                        </span>
                    ),
                },
            ];
        };
        /*
         * A split holds several panes in one tab, so the strip shows one tab per
         * pane, outlined together. The window's own tab stands for the pane it
         * was opened for; the panes brought in beside it get tabs of their own.
         */
        const grouped = (windowId: string, own: TabDescriptor[]): TabDescriptor[] => {
            const win = windowsById[windowId];
            const panes = win ? collectPanes(win.root).filter((pane) => pane.kind !== "desk") : [];
            if (!win || panes.length < 2) return own;
            const live = session.activeWindowId === win.id;
            const documentsPane = win.role === "files" ? editorPaneOf(win, editorViews) : null;
            const home = documentsPane ?? (panes.find((pane) => paneRole(pane.kind) === win.role) ?? panes[0]).id;
            return panes.flatMap((pane): TabDescriptor[] => {
                const focused = live && win.activePaneId === pane.id;
                if (pane.id === home) {
                    return own.map((tab) => {
                        paneOfTab.set(tab.id, { windowId: win.id, paneId: pane.id });
                        const kept = pane.id === documentsPane || pane.kind === "agent" ? {} : { ...look(pane, win), closable: true };
                        return { ...tab, ...kept, active: !!tab.active && win.activePaneId === pane.id, group: win.id };
                    });
                }
                const id = `${win.id}/${pane.id}`;
                paneOfTab.set(id, { windowId: win.id, paneId: pane.id });
                return [{ id, ...look(pane, win), active: focused, group: win.id }];
            });
        };
        const byWindow = new Map<string, TabRef[]>();
        for (const ref of refs) byWindow.set(ref.id, [...(byWindow.get(ref.id) ?? []), ref]);
        const tabs: WorkspaceTab[] = [...byWindow].flatMap(([windowId, windowRefs]) =>
            grouped(windowId, windowRefs.flatMap(build)).map((tab) => ({
                ...tab,
                tabId: `workspace-tab-${session.id}-${encodeURIComponent(tab.id)}`,
                panelId: `workspace-content-${session.id}`,
            })),
        );
        return { tabs, paneOfTab };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- a plugin's documents live outside the store
    }, [
        refs,
        windowsById,
        agentsById,
        agentStatus,
        termTitles,
        dirtyEditorPaths,
        activeKey,
        session.id,
        session.activeWindowId,
        editorViews,
        documentsVersion,
    ]);

    const refByKey = new Map(refs.map((ref) => [tabRefKey(ref), ref]));

    const atGroupEdge = (key: string, placement: "before" | "after") => {
        const group = tabs.find((tab) => tab.id === key)?.group;
        if (!group) return true;
        const members = tabs.filter((tab) => tab.group === group);
        return (placement === "before" ? members[0] : members[members.length - 1]).id === key;
    };
    // Beside a split tab means beside the whole group, so only its outer tabs take a drop.
    const dropTarget = (key: string, placement: "before" | "after"): TabRef | undefined => {
        if (!atGroupEdge(key, placement)) return undefined;
        const pane = paneOfTab.get(key);
        return refByKey.get(key) ?? (pane ? { id: pane.windowId } : undefined);
    };
    const leavingPane = (key: string) => {
        const pane = refByKey.has(key) ? undefined : paneOfTab.get(key);
        const win = pane ? windowsById[pane.windowId] : undefined;
        return pane && win && paneToSeparate(win, getState(), pane.paneId) ? pane : undefined;
    };

    const withSeparate = (win: WindowT, items: CtxItem[]): CtxItem[] => {
        const pane = paneToSeparate(win, getState());
        if (!pane) return items;
        const label = pane.kind === "editor" ? "Move Focused File Back to Editor" : "Move Focused Pane Back to Tab Bar";
        return [...items, { sep: true }, { label, run: () => cmd.separatePane(win.id) }];
    };

    const splittable = (key: string) => {
        const ref = refByKey.get(key);
        return !!ref && tabSplitAllowed(getState(), session.id, ref);
    };
    const dragOut: TabDragOut = {
        allows: splittable,
        hover: (_key, point) => setSplitTarget(point ? splitTargetAt(session.id, point) : null),
        drop: (key, point) => {
            const ref = refByKey.get(key);
            const target = splitTargetAt(session.id, point);
            if (ref && target) cmd.splitWithTab(session.id, ref, target.side, target.paneId);
        },
    };

    return (
        <>
            {splitTarget && <div className={`split-preview split-preview--${splitTarget.side}`} style={splitTarget.box} aria-hidden="true" />}
            <TabBar
                variant="agent"
                tabs={tabs}
                onSelect={(key) => {
                    const ref = refByKey.get(key);
                    const pane = paneOfTab.get(key);
                    if (ref) cmd.selectTab(ref);
                    else if (pane) cmd.selectTab({ id: pane.windowId });
                    if (pane) cmd.focusPane(pane.paneId);
                }}
                onKeep={(key) => {
                    const ref = refByKey.get(key);
                    const win = ref ? windowsById[ref.id] : undefined;
                    if (win && ref?.doc !== undefined) cmd.keepEditorTab(editorPaneOf(win, getState().editorViews), ref.doc);
                }}
                onRename={(key) => {
                    const win = windowsById[refByKey.get(key)?.id ?? paneOfTab.get(key)?.windowId ?? ""];
                    if (win?.role === "term") void renameTerminalTab(win, termTitles);
                    const agent = agentInWindow(agentsById, win);
                    if (agent) void (agent.name ? renameAgentNamePrompt(agent) : renameAgentPrompt(agent));
                }}
                onClose={(key) => {
                    const ref = refByKey.get(key);
                    const pane = paneOfTab.get(key);
                    if (pane && ref?.doc === undefined && windowsById[pane.windowId]?.role !== "agent") cmd.closePane(pane.windowId, pane.paneId);
                    else if (ref) cmd.closeTab(ref);
                }}
                buildMenu={(key) => {
                    const ref = refByKey.get(key);
                    const pane = paneOfTab.get(key);
                    if (!ref && pane) {
                        const win = windowsById[pane.windowId];
                        const movable = !!win && !!paneToSeparate(win, getState(), pane.paneId);
                        return [
                            {
                                label: "Move Back to Tab Bar",
                                icon: <IconMove size={13} />,
                                disabled: !movable,
                                run: () => cmd.separatePane(pane.windowId, pane.paneId),
                            },
                        ];
                    }
                    if (!ref) return [];
                    const win = windowsById[ref.id];
                    if (!win) return [];
                    const pluginDocs = ref.doc !== undefined ? pluginDocuments(win.role) : undefined;
                    if (ref.doc !== undefined && pluginDocs) {
                        const doc = ref.doc;
                        return pluginDocs.menu
                            ? [...pluginDocs.menu(win.activePaneId, doc)]
                            : [{ label: "Close", icon: <IconClose size={13} />, run: () => cmd.closeTab(ref) }];
                    }
                    if (ref.doc !== undefined) return fileTabMenu(session.cwd, win, ref.doc, closeShortcut);
                    if (win.role === "agent") {
                        const agent = agentInWindow(agentsById, win);
                        return agent ? withSeparate(win, agentTabMenu(agent)) : [];
                    }
                    return withSeparate(win, windowTabMenu(session.id, win, closeShortcut));
                }}
                onAdd={() => cmd.openNewTabPalette()}
                addIcon={<IconPlus size={13} />}
                addTitle="New tab"
                canReorder={(sourceKey, targetKey, placement) => {
                    const leaving = leavingPane(sourceKey);
                    const source = refByKey.get(sourceKey) ?? (leaving ? { id: leaving.windowId } : undefined);
                    const target = dropTarget(targetKey, placement);
                    return !!source && !!target && workspaceTabDropAllowed(refs, source, target, placement);
                }}
                onReorder={(sourceKey, targetKey, placement) => {
                    const target = dropTarget(targetKey, placement);
                    const leaving = leavingPane(sourceKey);
                    if (target && leaving) {
                        const before = new Set(getState().windowsBySession[session.id]);
                        cmd.separatePane(leaving.windowId, leaving.paneId);
                        const separated = getState().windowsBySession[session.id]?.find((id) => !before.has(id));
                        if (separated) cmd.reorderWindowTab(session.id, separated, target.id, placement);
                        return;
                    }
                    const source = refByKey.get(sourceKey);
                    if (!source || !target) return;
                    // Beside another window's documents means beside that window.
                    if (source.doc !== undefined && target.doc !== undefined) cmd.reorderDocumentTab(source.id, source.doc, target.doc, placement);
                    else cmd.reorderWindowTab(session.id, source.id, target.id, placement);
                }}
                dragOut={dragOut}
            />
        </>
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
    const editorPaneId = useStore((s) => editorPaneOf(win, s.editorViews));
    const editorView = useStore((s) => s.editorViews[editorPaneId]);
    usePluginDocumentsVersion();
    const editorViews = editorView ? { [editorPaneId]: editorView } : {};
    const active = activeTabRef(session, { [win.id]: win }, editorViews);
    const documents = documentsOf(win, editorViews);
    const layerRef = useRef<HTMLDivElement>(null);
    useDocumentSlide(layerRef, live ? editorPaneId : null, documents?.activeId ?? null, documents?.ids ?? EMPTY_IDS);
    const zoomedPaneId = useStore((s) => s.zoomedPaneId);
    const paneShader = useStore((s) => s.paneShader);
    const { panes, dividers, stacked, stacks, inStack } = useMemo(() => computeLayout(win.root, win.activePaneId), [win.root, win.activePaneId]);
    /*
     * Only the panes stacked behind a strip show a shell's own title, so only
     * those are subscribed to. Reading the whole title record here meant one
     * prompt or `cd` anywhere re-rendered every retained layer in the app, and
     * every pane inside them.
     */
    const stackPtyIds = useMemo(
        () => stacks.flatMap((stack) => stack.tabs.flatMap((pane) => (pane.ptyId !== undefined ? [String(pane.ptyId)] : []))),
        [stacks],
    );
    const stackTitleList = useStore(useShallow((s) => stackPtyIds.map((id) => s.terminalTitles[id] ?? "")));
    const stackTitles = useMemo(() => Object.fromEntries(stackPtyIds.map((id, index) => [id, stackTitleList[index]])), [stackPtyIds, stackTitleList]);
    const leaves = useMemo(() => collectPanes(win.root), [win.root]);
    /* Read as one flag per pane, so a file opened or edited in another window does not re-render this one. */
    const separable = useStore(useShallow((s) => leaves.map((leaf) => paneToSeparate(win, s, leaf.id) !== null)));
    const zoomActive = live && zoomedPaneId != null;

    return (
        <div
            ref={layerRef}
            className={`window-layer${live ? " live" : ""}${painted ? " painted" : ""}`}
            data-window-id={win.id}
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
                const canUnsplit = separable[leaves.indexOf(p)] || (p.kind === "agent" && separable.includes(true));
                return (
                    <div
                        key={p.id}
                        className={`pane-cell${inStack.has(p.id) ? " in-stack" : ""}${shown ? "" : " unshown"}`}
                        data-pane-cell={p.id}
                        style={{
                            ...cellPlacement(rect),
                            visibility: shown ? undefined : "hidden",
                            zIndex: isZoomed ? 2 : 1,
                        }}>
                        <div
                            className={`pane pane-${isPluginKind(p.kind) ? "plugin" : p.kind}`}
                            data-pane-id={p.id}
                            onMouseDown={() => live && cmd.focusPane(p.id)}>
                            <PanePaintedContext.Provider value={panePainted}>
                                {/* The pane is a surface, so it carries its own texture — and only
                                    while its screen is on stage, so a screen off stage spends no
                                    WebGL context on a field nobody is looking at. The editor draws
                                    its own, on the code panel beside its file tree, git on its review column, a plugin only on its sign-in, and the desk none. */}
                                {p.kind !== "editor" && p.kind !== "git" && p.kind !== "desk" && !isPluginKind(p.kind) && (
                                    <PaneField enabled={paneShader} />
                                )}
                                <ScreenFieldContext.Provider value={isPluginKind(p.kind)}>
                                    <ErrorBoundary label={`${p.kind} pane`}>
                                        {renderWorkbenchItem({
                                            pane: p,
                                            session,
                                            win,
                                            active: paneActive,
                                            visible: paneVisible,
                                            painted: panePainted,
                                        })}
                                    </ErrorBoundary>
                                </ScreenFieldContext.Provider>
                            </PanePaintedContext.Provider>
                            {live && canUnsplit && (
                                <button
                                    type="button"
                                    className="pane-unsplit"
                                    aria-label={p.kind === "editor" ? "Move file back to the editor" : "Move back to the tab bar"}
                                    title={p.kind === "editor" ? "Move file back to the editor" : "Move back to the tab bar"}
                                    onMouseDown={(event) => event.stopPropagation()}
                                    onClick={() => (p.kind === "agent" ? cmd.unsplitTab(win.id) : cmd.separatePane(win.id, p.id))}>
                                    <IconArrowUp size={12} />
                                    <span>{p.kind === "editor" ? "Back to editor" : "Move to tab bar"}</span>
                                </button>
                            )}
                        </div>
                    </div>
                );
            })}
            {!zoomActive &&
                stacks.map((stack) => (
                    <div key={stack.splitId} className="stack-strip" data-stack={stack.splitId} style={stackPlacement(stack.rect)}>
                        <TabBar
                            variant="stack"
                            ariaLabel="Panes in this stack"
                            tabs={stack.tabs.map((pane) => ({
                                id: pane.id,
                                label: terminalLabel(win, pane, stackTitles),
                                title: terminalLabel(win, pane, stackTitles),
                                active: pane.id === stack.activePaneId,
                                icon: (
                                    <span className={`agent-glyph ${paneRole(pane.kind)}`}>
                                        <WindowIcon role={paneRole(pane.kind)} size={12} />
                                    </span>
                                ),
                                closable: false,
                            }))}
                            onSelect={(paneId) => live && cmd.focusPane(paneId)}
                        />
                    </div>
                ))}
            {live && !zoomActive && dividers.map((d) => <DividerHandle key={dividerKey(d)} d={d} windowId={win.id} areaRef={areaRef} />)}
        </div>
    );
});

function DividerHandle({ d, windowId, areaRef }: { d: Divider; windowId: string; areaRef: RefObject<HTMLDivElement | null> }) {
    const horizontal = d.dir === "row";

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
        // Stopping the press's default also stops it focusing, and focus is what lets the arrow keys carry on.
        handle.focus({ preventScroll: true });
        handle.classList.add("dragging", "pointer-focus");

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
            handle.classList.remove("dragging");
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", up);
            handle.removeEventListener("pointercancel", up);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
        handle.addEventListener("pointercancel", up);
    };

    const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
        e.currentTarget.classList.remove("pointer-focus");
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

    const evenOut = () => {
        const winNode = getState().windows[windowId];
        const split = winNode ? findSplit(winNode.root, d.splitId) : null;
        if (split)
            cmd.setSplitSizes(
                windowId,
                d.splitId,
                split.children.map(() => 1 / split.children.length),
            );
    };

    return (
        <div
            className={`divider divider-${d.dir}`}
            data-divider={dividerKey(d)}
            style={dividerPlacement(d)}
            role="separator"
            tabIndex={0}
            aria-orientation={horizontal ? "vertical" : "horizontal"}
            title="Drag or use arrow keys to resize, double-click to even out"
            onPointerDown={onPointerDown}
            onKeyDown={onKeyDown}
            onBlur={(e) => e.currentTarget.classList.remove("pointer-focus")}
            onDoubleClick={evenOut}
        />
    );
}
