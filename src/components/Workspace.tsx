import { lazy, memo, Suspense, useEffect, useMemo, useRef } from "react";
import type { PointerEvent as ReactPointerEvent, RefObject } from "react";
import type { Agent, Divider, Rect, Session, Window as WindowT } from "../state/types";
import { collectPanes, computeLayout, findSplit, MIN_FRAC } from "../state/layout";
import * as cmd from "../state/commands";
import { getState, useStore } from "../state/store";
import { type CtxItem } from "./FileTree";
import { ErrorBoundary } from "./ErrorBoundary";
import { TabBar, type TabDescriptor } from "./TabBar";
import { AgentIcon, IconCommand, IconGlobe, IconPlus } from "./Icons";
import { renderWorkbenchItem } from "../workbench/renderers";
import { AgentBrowserShell } from "./BrowserPane";

const AgentSurface = lazy(() => import("../chat/AgentSurface").then((module) => ({ default: module.AgentSurface })));

const AGENT_TABS_H = 34;
const TERM_TABS_H = 34;

const FULL: Rect = { x: 0, y: 0, w: 1, h: 1 };
const pct = (n: number) => `${n * 100}%`;
// Agents only exist in project sessions; every other group is always in
// "windows" view, so stale agent state can never strand a non-project session.
const sessionView = (s: Session): "windows" | "agent" => (s.kind === "project" ? s.view : "windows");

export function Workspace() {
    const sessionsById = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const windowsById = useStore((s) => s.windows);
    const agentsById = useStore((s) => s.agents);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agentsBySession = useStore((s) => s.agentsBySession);
    const activeSessionId = useStore((s) => s.activeSessionId);
    const agentPaletteOpen = useStore((s) => s.agentPaletteOpen);
    const areaRef = useRef<HTMLDivElement>(null);

    const sessions = sessionOrder.map((id) => sessionsById[id]);
    const activeSession = sessionsById[activeSessionId];
    const activeWindow = activeSession ? windowsById[activeSession.activeWindowId] : undefined;
    const activeAgents = activeSession ? (agentsBySession[activeSession.id] ?? []).map((id) => agentsById[id]) : [];

    const inAgentView = !!activeSession && sessionView(activeSession) === "agent";
    const showAgentTabs = inAgentView && activeAgents.length > 0;
    const showAgentEmpty = inAgentView && activeAgents.length === 0;

    useEffect(() => {
        if (showAgentEmpty && !agentPaletteOpen) cmd.openAgentPalette();
    }, [agentPaletteOpen, activeSessionId, showAgentEmpty]);

    const activeWindowList = activeSession ? (windowsBySession[activeSession.id] ?? []).map((id) => windowsById[id]) : [];
    const termTabs =
        activeSession && sessionView(activeSession) === "windows" && activeWindow?.role === "term"
            ? activeWindowList.filter((w) => w.role === "term")
            : [];
    const showTermTabs = termTabs.length > 0;

    return (
        <div className="window-area" ref={areaRef}>
            {showAgentTabs && <AgentTabsBar session={activeSession!} agents={activeAgents} />}
            {showTermTabs && <TerminalTabsBar session={activeSession!} tabs={termTabs} />}
            {showAgentEmpty && (
                <div className="agent-empty-stage">
                    <span>no agents in this project</span>
                    <span className="agent-empty-hint">start one from the agent rail →</span>
                </div>
            )}
            {sessions.flatMap((session) => {
                const isActive = session.id === activeSessionId;
                const view = sessionView(session);
                const winIds = windowsBySession[session.id] ?? [];
                const aIds = agentsBySession[session.id] ?? [];
                const sessTabs = view === "agent" && aIds.length > 0;
                const sessHasTermTabs = winIds.some((id) => windowsById[id]?.role === "term");
                const renderedWindowIds = isActive && view === "windows" ? [session.activeWindowId] : [];
                const windowLayers = renderedWindowIds.map((wid) => {
                    const win = windowsById[wid];
                    if (!win) return null;
                    const layerTermTab = win.role === "term";
                    const inset = isActive && view === "windows" && wid === session.activeWindowId && layerTermTab && sessHasTermTabs;
                    return (
                        <WindowLayer
                            key={wid}
                            session={session}
                            win={win}
                            areaRef={areaRef}
                            topInset={inset ? TERM_TABS_H : 0}
                            visible={isActive && view === "windows" && wid === session.activeWindowId}
                        />
                    );
                });
                const agentLayers = aIds.map((aid) => {
                    const agent = agentsById[aid];
                    if (!agent) return null;
                    const visible = isActive && view === "agent" && aid === session.activeAgentId;
                    if (!visible && agent.launchState === "dormant") return null;
                    return <AgentLayer key={aid} session={session} agent={agent} tabsShown={sessTabs} visible={visible} />;
                });
                return [...windowLayers, ...agentLayers];
            })}
        </div>
    );
}

function TerminalTabsBar({ session, tabs }: { session: Session; tabs: WindowT[] }) {
    const terminalTitles = useStore((s) => s.terminalTitles);
    const buildMenu = (id: string): CtxItem[] => {
        const w = tabs.find((t) => t.id === id);
        if (!w) return [];
        const others = tabs.filter((t) => t.id !== id && !t.fixed);
        const all = tabs.filter((t) => !t.fixed);
        return [
            { label: "Duplicate", run: () => cmd.duplicateWindow(id) },
            { label: "Close", hint: "⌥W", disabled: w.fixed, run: () => cmd.closeWindowById(id) },
            { label: "Close Others", disabled: others.length === 0, run: () => others.forEach((t) => cmd.closeWindowById(t.id)) },
            { label: "Close All", disabled: all.length === 0, run: () => all.forEach((t) => cmd.closeWindowById(t.id)) },
        ];
    };

    return (
        <TabBar
            variant="agent"
            style={{ height: TERM_TABS_H }}
            tabs={tabs.map((w) => ({
                id: w.id,
                label: terminalTitles[w.activePaneId] || w.name,
                title: terminalTitles[w.activePaneId] || w.name,
                active: w.id === session.activeWindowId,
                closable: !w.fixed,
                icon: (
                    <span className="agent-glyph">
                        <IconCommand size={13} />
                    </span>
                ),
            }))}
            onSelect={cmd.selectWindowId}
            onClose={cmd.closeWindowById}
            buildMenu={buildMenu}
            onAdd={() => cmd.newWindow()}
            addIcon={<IconPlus size={13} />}
            addTitle="New terminal — ⌥N"
        />
    );
}

function AgentTabsBar({ session, agents }: { session: Session; agents: Agent[] }) {
    const buildMenu = (id: string): CtxItem[] => {
        const a = agents.find((x) => x.id === id);
        if (!a) return [];
        const others = agents.filter((x) => x.id !== id);
        const items: CtxItem[] = [
            ...(a.launchState === "dormant"
                ? [{ label: "Resume", run: () => cmd.selectAgent(id) }]
                : a.resumeId
                  ? [{ label: "Sleep", run: () => cmd.sleepAgent(id) }]
                  : []),
            ...(a.resumeId && a.launchState !== "dormant"
                ? [
                      {
                          label: a.keepAlive ? "Allow Auto-Sleep" : "Keep Alive",
                          run: () => cmd.setAgentKeepAlive(id, !a.keepAlive),
                      },
                  ]
                : []),
            ...(a.resumeId ? [{ sep: true as const }] : []),
            { label: "Close", hint: "⌥W", run: () => cmd.closeAgent(id) },
            { label: "Close Others", disabled: others.length === 0, run: () => others.forEach((x) => cmd.closeAgent(x.id)) },
            { label: "Close All", run: () => agents.forEach((x) => cmd.closeAgent(x.id)) },
        ];
        if (cmd.agentSupportsSkipPermissions(a.type)) {
            const skip = a.permissionMode === "bypass" || a.skipPermissions === true;
            items.push(
                { sep: true },
                {
                    label: skip ? "Disable YOLO Mode" : "Enable YOLO Mode",
                    hint: "⌥Y",
                    run: () => cmd.toggleAgentSkipPermissions(id),
                },
            );
        }
        return items;
    };

    const tabs: TabDescriptor[] = agents.map((a) => ({
        id: a.id,
        label: a.title,
        title: a.title,
        active: a.id === session.activeAgentId,
        icon: (
            <span className={`agent-glyph ${a.type}`}>
                <AgentIcon type={a.type} size={14} />
            </span>
        ),
    }));
    return (
        <TabBar
            variant="agent"
            style={{ height: AGENT_TABS_H }}
            tabs={tabs}
            onSelect={cmd.selectAgent}
            onClose={cmd.closeAgent}
            buildMenu={buildMenu}
            onAdd={() => cmd.openAgentPalette()}
            addIcon={<IconPlus size={13} />}
            addTitle="New agent — ⌥N"
            trailing={
                <button
                    type="button"
                    className="agent-browser-open"
                    aria-label="New browser tab — ⌘T"
                    title="New browser tab — ⌘T"
                    onClick={cmd.newBrowserTab}>
                    <IconGlobe size={13} />
                    <span>browser</span>
                </button>
            }
        />
    );
}

const AgentLayer = memo(function AgentLayer({
    session,
    agent,
    visible,
    tabsShown,
}: {
    session: Session;
    agent: Agent;
    visible: boolean;
    tabsShown: boolean;
}) {
    const profile = useStore((state) => (agent.profileId ? state.providerProfiles.find((candidate) => candidate.id === agent.profileId) : undefined));
    return (
        <div className={`window-layer${visible ? " visible" : ""}`} aria-hidden={!visible} inert={!visible}>
            <div
                className="pane-cell"
                style={{
                    left: 0,
                    top: tabsShown ? `${AGENT_TABS_H}px` : 0,
                    width: "100%",
                    height: tabsShown ? `calc(100% - ${AGENT_TABS_H}px)` : "100%",
                }}>
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
