import { lazy, memo, Suspense, useMemo, useRef } from "react";
import type { PointerEvent as ReactPointerEvent, RefObject } from "react";
import type { Agent, Divider, Rect, Session, Window as WindowT, WindowRole, WorkspaceTabRef } from "../state/types";
import { collectPanes, computeLayout, findSplit, MIN_FRAC } from "../state/layout";
import * as cmd from "../state/commands";
import { getState, useStore } from "../state/store";
import { activeTabRef, expandTabRefs, roleHasTab, tabRefKey } from "../state/selectors";
import { type CtxItem } from "./FileTree";
import { ErrorBoundary } from "./ErrorBoundary";
import { TabBar, type TabDescriptor } from "./TabBar";
import { AgentIcon, IconCommand, IconGlobe, IconPlus, WindowIcon } from "./Icons";
import { AgentStateIndicator } from "./AgentStateIndicator";
import { renderWorkbenchItem } from "../workbench/renderers";
import { AgentBrowserShell } from "./BrowserPane";
import { ShaderField } from "./ShaderField";
import { FileIcon } from "./FileIcon";
import { fsapi } from "../api/fs";
import { basename, relativePath } from "../lib/paths";
import { FILE_MANAGER_NAME, PRIMARY_SHORTCUT } from "../lib/platform";
import { notify, reportError } from "../state/toast";

const copyPath = (_path: string, text: string, label: string) =>
    navigator.clipboard.writeText(text).then(() => notify("success", `copied ${label}`), reportError("copy"));

const AgentSurface = lazy(() => import("../chat/AgentSurface").then((module) => ({ default: module.AgentSurface })));

const TABS_H = 34;

const FULL: Rect = { x: 0, y: 0, w: 1, h: 1 };
const pct = (n: number) => `${n * 100}%`;
export function Workspace() {
    const sessionsById = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const windowsById = useStore((s) => s.windows);
    const agentsById = useStore((s) => s.agents);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agentsBySession = useStore((s) => s.agentsBySession);
    const activeSessionId = useStore((s) => s.activeSessionId);
    const areaRef = useRef<HTMLDivElement>(null);

    const sessions = sessionOrder.map((id) => sessionsById[id]);
    const activeSession = sessionsById[activeSessionId];
    // Counts what the strip would actually show: a project holding only
    // rail-driven surfaces has no tabs, and no strip.
    const tabCount = activeSession
        ? (windowsBySession[activeSession.id] ?? []).filter((id) => {
              const role = windowsById[id]?.role;
              return role !== undefined && roleHasTab(role);
          }).length + (agentsBySession[activeSession.id]?.length ?? 0)
        : 0;

    return (
        <div className="window-area" ref={areaRef}>
            {/*
             * The content area's one surface. Mounted here rather than per pane
             * so it spans the tab strip and every pane as a single field, holds
             * one WebGL context no matter how the window is split, and survives
             * tab switches instead of being torn down and rebuilt with them.
             * Panes and the tab strip draw over it; none of them paint a ground
             * of their own any more.
             */}
            <ShaderField preset="pane" className="stage-field" />
            {activeSession && tabCount > 0 && <WorkspaceTabsBar session={activeSession} />}
            {sessions.flatMap((session) => {
                const isActive = session.id === activeSessionId;
                const active = activeTabRef(session);
                const winIds = windowsBySession[session.id] ?? [];
                const aIds = agentsBySession[session.id] ?? [];
                const windowLayers = winIds.map((wid) => {
                    const win = windowsById[wid];
                    if (!win) return null;
                    const visible = isActive && active?.kind === "window" && active.id === wid;
                    if (!visible && wid !== session.activeWindowId) return null;
                    return <WindowLayer key={wid} session={session} win={win} areaRef={areaRef} topInset={TABS_H} visible={visible} />;
                });
                const agentLayers = aIds.map((aid) => {
                    const agent = agentsById[aid];
                    if (!agent) return null;
                    const visible = isActive && active?.kind === "agent" && active.id === aid;
                    if (!visible && agent.launchState === "dormant") return null;
                    return <AgentLayer key={aid} session={session} agent={agent} visible={visible} />;
                });
                return [...windowLayers, ...agentLayers];
            })}
        </div>
    );
}

const EMPTY_IDS: readonly string[] = [];

const ROLE_LABEL: Record<WindowRole, string> = {
    term: "Terminal",
    files: "Editor",
    git: "Git",
    diff: "Diff",
    search: "Search",
    aws: "AWS",
    rundeck: "Rundeck",
    bruno: "Bruno",
    "ssh-config": "SSH config",
    named: "Window",
};

function WorkspaceTabsBar({ session }: { session: Session }) {
    const windowsById = useStore((s) => s.windows);
    const agentsById = useStore((s) => s.agents);
    const terminalTitles = useStore((s) => s.terminalTitles);
    const activity = useStore((s) => s.agentActivity);
    const windowIds = useStore((s) => s.windowsBySession[session.id]);
    const agentIds = useStore((s) => s.agentsBySession[session.id]);
    const editorViews = useStore((s) => s.editorViews);
    const dirtyEditorPaths = useStore((s) => s.dirtyEditorPaths);
    // Shared with cycleTab through selectTabRefs, so the strip and the keyboard
    // can never disagree about what the tabs are.
    const refs = useMemo(
        () => expandTabRefs(windowIds ?? EMPTY_IDS, agentIds ?? EMPTY_IDS, windowsById, agentsById, editorViews),
        [windowIds, agentIds, windowsById, agentsById, editorViews],
    );
    const active = activeTabRef(session, windowsById, editorViews);
    const activeKey = active ? tabRefKey(active) : null;

    const windowMenu = (win: WindowT): CtxItem[] => {
        const siblings = refs.flatMap((ref) => (ref.kind === "window" ? [windowsById[ref.id]] : [])).filter(Boolean) as WindowT[];
        const others = siblings.filter((t) => t.id !== win.id && !t.fixed);
        return [
            { label: "Duplicate", run: () => cmd.duplicateWindow(win.id) },
            { label: "Close", hint: "⌥W", disabled: win.fixed, run: () => cmd.closeWindowById(win.id) },
            { label: "Close Others", disabled: others.length === 0, run: () => others.forEach((t) => cmd.closeWindowById(t.id)) },
        ];
    };

    const fileMenu = (ref: Extract<WorkspaceTabRef, { kind: "file" }>): CtxItem[] => {
        const win = windowsById[ref.id];
        const open = win ? (editorViews[win.activePaneId]?.openTabs ?? []) : [];
        const dirty = new Set(win ? (dirtyEditorPaths[win.activePaneId] ?? []) : []);
        const index = open.indexOf(ref.path);
        const close = (paths: string[]) => paths.forEach((path) => cmd.closeTab({ kind: "file", id: ref.id, path }));
        const others = open.filter((path) => path !== ref.path);
        const toLeft = index > 0 ? open.slice(0, index) : [];
        const toRight = index >= 0 ? open.slice(index + 1) : [];
        const saved = open.filter((path) => !dirty.has(path));
        return [
            { label: "Close", hint: `${PRIMARY_SHORTCUT}W`, run: () => close([ref.path]) },
            { label: "Close Others", disabled: others.length === 0, run: () => close(others) },
            { label: "Close to the Left", disabled: toLeft.length === 0, run: () => close(toLeft) },
            { label: "Close to the Right", disabled: toRight.length === 0, run: () => close(toRight) },
            { label: "Close Saved", disabled: saved.length === 0, run: () => close(saved) },
            { label: "Close All", run: () => close(open) },
            { sep: true },
            { label: "Copy Path", run: () => void copyPath(ref.path, ref.path, "path") },
            {
                label: "Copy Relative Path",
                run: () => void copyPath(ref.path, relativePath(ref.path, session.cwd) ?? basename(ref.path), "relative path"),
            },
            { sep: true },
            { label: `Reveal in ${FILE_MANAGER_NAME}`, run: () => void fsapi.revealInFinder(ref.path).catch(reportError("reveal")) },
        ];
    };

    const agentMenu = (agent: Agent): CtxItem[] => {
        const agents = refs.flatMap((ref) => (ref.kind === "agent" ? [agentsById[ref.id]] : [])).filter(Boolean) as Agent[];
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

    const tabs: TabDescriptor[] = refs.flatMap((ref): TabDescriptor[] => {
        const key = tabRefKey(ref);
        if (ref.kind === "agent") {
            const agent = agentsById[ref.id];
            if (!agent) return [];
            const state = activity[agent.id];
            return [
                {
                    id: key,
                    label: agent.title,
                    title: agent.title,
                    active: key === activeKey,
                    icon: (
                        <span className={`agent-glyph ${agent.type}`}>
                            <AgentIcon type={agent.type} size={14} />
                        </span>
                    ),
                    accessory: state ? <AgentStateIndicator state={state.state} /> : undefined,
                },
            ];
        }
        if (ref.kind === "file") {
            const win = windowsById[ref.id];
            if (!win) return [];
            const name = basename(ref.path);
            return [
                {
                    id: key,
                    label: name,
                    title: ref.path,
                    active: key === activeKey,
                    dirty: (dirtyEditorPaths[win.activePaneId] ?? []).includes(ref.path),
                    icon: <FileIcon name={name} size={16} />,
                },
            ];
        }
        const win = windowsById[ref.id];
        if (!win) return [];
        const label = win.role === "term" ? terminalTitles[win.activePaneId] || win.name : ROLE_LABEL[win.role];
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
    });

    const refByKey = new Map(refs.map((ref) => [tabRefKey(ref), ref]));

    return (
        <TabBar
            variant="agent"
            style={{ height: TABS_H }}
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
                if (ref.kind === "file") return fileMenu(ref);
                if (ref.kind === "agent") {
                    const agent = agentsById[ref.id];
                    return agent ? agentMenu(agent) : [];
                }
                const win = windowsById[ref.id];
                return win ? windowMenu(win) : [];
            }}
            onAdd={() => cmd.openAgentPalette()}
            addIcon={<IconPlus size={13} />}
            addTitle="New agent"
            trailing={
                <>
                    <button
                        type="button"
                        className="agent-browser-open"
                        aria-label="New terminal"
                        title="New terminal"
                        onClick={() => cmd.newWindow()}>
                        <IconCommand size={13} />
                        <span>term</span>
                    </button>
                    <button
                        type="button"
                        className="agent-browser-open"
                        aria-label="New browser tab — ⌘T"
                        title="New browser tab — ⌘T"
                        onClick={cmd.newBrowserTab}>
                        <IconGlobe size={13} />
                        <span>browser</span>
                    </button>
                </>
            }
        />
    );
}

const AgentLayer = memo(function AgentLayer({ session, agent, visible }: { session: Session; agent: Agent; visible: boolean }) {
    const profile = useStore((state) => (agent.profileId ? state.providerProfiles.find((candidate) => candidate.id === agent.profileId) : undefined));
    return (
        <div className={`window-layer${visible ? " visible" : ""}`} aria-hidden={!visible} inert={!visible}>
            <div className="pane-cell" style={{ left: 0, top: `${TABS_H}px`, width: "100%", height: `calc(100% - ${TABS_H}px)` }}>
                <div className="pane pane-terminal">
                    <AgentBrowserShell agentId={agent.id} agentType={agent.type} visible={visible}>
                        {agent.launchState === "dormant" ? (
                            <div className="agent-dormant" role="group" aria-label={`${agent.title} is ready to resume`}>
                                <span className={`agent-dormant-notch ${agent.type}`} aria-hidden="true" />
                                <span className="agent-dormant-kicker">sleeping</span>
                                <strong>{agent.title}</strong>
                                <span>This resumable agent is using no live terminal process.</span>
                                <button type="button" onClick={() => cmd.resumeAgent(agent.id)}>
                                    Resume {agent.type}
                                </button>
                            </div>
                        ) : (
                            <Suspense fallback={<div className="agent-transport-switching">Opening agent session…</div>}>
                                <AgentSurface agent={agent} session={session} profile={profile} visible={visible} />
                            </Suspense>
                        )}
                    </AgentBrowserShell>
                </div>
            </div>
        </div>
    );
});

const WindowLayer = memo(function WindowLayer({
    session,
    win,
    visible,
    areaRef,
    topInset = 0,
}: {
    session: Session;
    win: WindowT;
    visible: boolean;
    areaRef: RefObject<HTMLDivElement | null>;
    topInset?: number;
}) {
    const zoomedPaneId = useStore((s) => s.zoomedPaneId);
    const { panes, dividers } = useMemo(() => computeLayout(win.root), [win.root]);
    const leaves = useMemo(() => collectPanes(win.root), [win.root]);
    const zoomActive = visible && zoomedPaneId != null;

    return (
        <div className={`window-layer${visible ? " visible" : ""}`} style={topInset ? { top: `${topInset}px` } : undefined}>
            {leaves.map((p) => {
                const isZoomed = zoomedPaneId === p.id;
                const shown = !zoomActive || isZoomed;
                const rect = isZoomed ? FULL : panes.get(p.id)!;
                const isActive = p.id === win.activePaneId;
                const paneVisible = visible && shown;
                const paneActive = paneVisible && isActive;
                return (
                    <div
                        key={p.id}
                        className="pane-cell"
                        style={{
                            left: pct(rect.x),
                            top: pct(rect.y),
                            width: pct(rect.w),
                            height: pct(rect.h),
                            visibility: shown ? undefined : "hidden",
                            zIndex: isZoomed ? 2 : 1,
                        }}>
                        <div className={`pane pane-${p.kind}`} onMouseDown={() => visible && cmd.focusPane(p.id)}>
                            <ErrorBoundary label={`${p.kind} pane`}>
                                {renderWorkbenchItem({ pane: p, session, win, active: paneActive, visible: paneVisible })}
                            </ErrorBoundary>
                        </div>
                    </div>
                );
            })}
            {visible &&
                !zoomActive &&
                dividers.map((d) => <DividerHandle key={`${d.splitId}:${d.index}`} d={d} windowId={win.id} areaRef={areaRef} />)}
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
