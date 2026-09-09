import { useCallback, useEffect, useLayoutEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { createPortal, flushSync } from "react-dom";
import { keybindingLabelForAction, type KeybindingActionId } from "../keybindings";
import type { Session, SessionKind, Window, WindowRole } from "../state/types";
import * as cmd from "../state/commands";
import { rollupAgentStates } from "../state/agentStatus";
import { getState, useStore } from "../state/store";
import {
    AgentIcon,
    IconAgent,
    IconAws,
    IconBruno,
    IconClose,
    IconCommand,
    IconFolder,
    IconPencil,
    IconPlus,
    IconRundeck,
    Logo,
    WindowIcon,
} from "./Icons";
import { Tooltip } from "./Tooltip";
import { EmptyState, Panel, PanelHeader } from "./Panel";
import { UpdateChip, VersionChip } from "./TopBar";
import { AgentStateIndicator } from "./AgentStateIndicator";

function kindIcon(kind: SessionKind): ReactNode {
    if (kind === "project") return <IconFolder size={13} />;
    if (kind === "aws") return <IconAws size={26} />;
    if (kind === "rundeck") return <IconRundeck size={14} />;
    if (kind === "bruno") return <IconBruno size={14} />;
    return <IconCommand size={13} />;
}

const MAX_BADGE_ICONS = 3;
type ProjectDropPlacement = "before" | "after";

interface ProjectDragSession {
    sourceId: string;
    startX: number;
    startY: number;
    grabX: number;
    grabY: number;
    width: number;
    height: number;
    sourceRow: HTMLElement;
    active: boolean;
    sequence: number;
}

interface ProjectDragVisual {
    width: number;
    height: number;
    grabX: number;
    grabY: number;
    row: HTMLElement;
}

function cloneProjectRow(source: HTMLElement): HTMLElement {
    const clone = source.cloneNode(true) as HTMLElement;
    const sourceElements = [source, ...source.querySelectorAll<HTMLElement>("*")];
    const cloneElements = [clone, ...clone.querySelectorAll<HTMLElement>("*")];
    sourceElements.forEach((element, index) => {
        const styles = window.getComputedStyle(element);
        for (let styleIndex = 0; styleIndex < styles.length; styleIndex += 1) {
            const property = styles.item(styleIndex);
            cloneElements[index].style.setProperty(property, styles.getPropertyValue(property), styles.getPropertyPriority(property));
        }
        cloneElements[index].style.setProperty("pointer-events", "none", "important");
    });
    clone.classList.add("project-drag-ghost-row");
    clone.removeAttribute("data-project-drop-row");
    clone.removeAttribute("aria-grabbed");
    clone.setAttribute("tabindex", "-1");
    return clone;
}

function projectElement(id: string): HTMLElement | null {
    return Array.from(document.querySelectorAll<HTMLElement>("[data-project-id]")).find((element) => element.dataset.projectId === id) ?? null;
}

function projectRects(): Map<string, DOMRect> {
    return new Map(
        Array.from(document.querySelectorAll<HTMLElement>("[data-project-id]"), (element) => [
            element.dataset.projectId ?? "",
            element.getBoundingClientRect(),
        ]),
    );
}

function reducedMotion(): boolean {
    return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

export function SideRail() {
    const sessionsById = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const windowsById = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agentsBySession = useStore((s) => s.agentsBySession);
    const agentsById = useStore((s) => s.agents);
    const activityById = useStore((s) => s.agentActivity);
    const rawActiveSessionId = useStore((s) => s.activeSessionId);
    const settingsOpen = useStore((s) => s.settingsOpen);
    const keybindingOverrides = useStore((s) => s.keybindingOverrides);
    const activeSessionId = settingsOpen ? "" : rawActiveSessionId;
    const kb = (id: KeybindingActionId) => keybindingLabelForAction(keybindingOverrides, id);
    const sessions = sessionOrder.map((id) => sessionsById[id]);
    const [draggingProjectId, setDraggingProjectId] = useState<string | null>(null);
    const [projectDrop, setProjectDrop] = useState<{ targetId: string; placement: ProjectDropPlacement } | null>(null);
    const [projectDragVisual, setProjectDragVisual] = useState<ProjectDragVisual | null>(null);
    const projectDragRef = useRef<ProjectDragSession | null>(null);
    const projectMoveHandlerRef = useRef<((event: PointerEvent) => void) | null>(null);
    const projectUpHandlerRef = useRef<((event: PointerEvent) => void) | null>(null);
    const projectGhostRef = useRef<HTMLDivElement | null>(null);
    const projectGhostPointRef = useRef<{ x: number; y: number } | null>(null);
    const projectDragSequenceRef = useRef(0);
    const suppressProjectClickRef = useRef(false);

    const projects = sessions.filter((s) => s.kind === "project");
    const sshs = sessions.filter((s) => s.kind === "ssh");
    const cloud = sessions.filter((s) => s.kind === "aws");
    const cicd = sessions.filter((s) => s.kind === "rundeck");
    const apis = sessions.filter((s) => s.kind === "bruno");
    const commands = sessions.filter((s) => s.kind === "command");

    const resolveProjectDrop = useCallback((x: number, y: number) => {
        const ghost = projectGhostRef.current;
        const previousVisibility = ghost?.style.visibility ?? "";
        if (ghost) ghost.style.visibility = "hidden";
        let hit: HTMLElement | null;
        try {
            hit = document.elementFromPoint(x, y) as HTMLElement | null;
        } finally {
            if (ghost) ghost.style.visibility = previousVisibility;
        }
        const target = hit?.closest<HTMLElement>("[data-project-id]");
        const targetId = target?.dataset.projectId;
        if (!targetId || targetId === projectDragRef.current?.sourceId) return null;
        const row = target.querySelector<HTMLElement>("[data-project-drop-row]");
        if (!row) return null;
        const bounds = row.getBoundingClientRect();
        return { targetId, placement: y < bounds.top + bounds.height / 2 ? ("before" as const) : ("after" as const) };
    }, []);

    const moveProjectGhost = useCallback((drag: ProjectDragSession, x: number, y: number) => {
        projectGhostPointRef.current = { x, y };
        if (projectGhostRef.current) {
            projectGhostRef.current.style.transform = `translate3d(${x - drag.grabX}px, ${y - drag.grabY}px, 0)`;
        }
    }, []);

    const animateProjectOrder = useCallback((sourceId: string, drop: { targetId: string; placement: ProjectDropPlacement }) => {
        const before = reducedMotion() ? null : projectRects();
        const previousOrder = getState().sessionOrder;
        flushSync(() => cmd.reorderSession(sourceId, drop.targetId, drop.placement));
        if (!before || getState().sessionOrder === previousOrder) return;
        window.requestAnimationFrame(() => {
            for (const [id, previous] of before) {
                const element = projectElement(id);
                if (!element || typeof element.animate !== "function") continue;
                const shift = previous.top - element.getBoundingClientRect().top;
                if (Math.abs(shift) < 1) continue;
                element.animate([{ transform: `translate3d(0, ${shift}px, 0)` }, { transform: "translate3d(0, 0, 0)" }], {
                    duration: 220,
                    easing: "cubic-bezier(0.2, 0.8, 0.2, 1)",
                });
            }
        });
    }, []);

    const endProjectDrag = useCallback((clearGhost = true) => {
        if (projectMoveHandlerRef.current) window.removeEventListener("pointermove", projectMoveHandlerRef.current);
        if (projectUpHandlerRef.current) window.removeEventListener("pointerup", projectUpHandlerRef.current);
        document.body.classList.remove("is-sorting-projects");
        projectMoveHandlerRef.current = null;
        projectUpHandlerRef.current = null;
        projectDragRef.current = null;
        setDraggingProjectId(null);
        setProjectDrop(null);
        if (clearGhost) {
            projectDragSequenceRef.current += 1;
            projectGhostPointRef.current = null;
            setProjectDragVisual(null);
        }
    }, []);

    const settleProjectGhost = useCallback((drag: ProjectDragSession) => {
        const ghost = projectGhostRef.current;
        const destination = projectElement(drag.sourceId)?.querySelector<HTMLElement>("[data-project-drop-row]");
        if (!ghost || !destination || reducedMotion() || typeof ghost.animate !== "function") {
            projectGhostPointRef.current = null;
            setProjectDragVisual(null);
            return;
        }

        const bounds = destination.getBoundingClientRect();
        const sequence = drag.sequence;
        const animation = ghost.animate(
            [
                { transform: ghost.style.transform, opacity: 1 },
                { transform: `translate3d(${bounds.left}px, ${bounds.top}px, 0)`, opacity: 0 },
            ],
            { duration: 180, easing: "cubic-bezier(0.22, 1, 0.36, 1)", fill: "forwards" },
        );
        void animation.finished
            .catch(() => undefined)
            .then(() => {
                if (projectDragSequenceRef.current !== sequence) return;
                projectGhostPointRef.current = null;
                setProjectDragVisual(null);
            });
    }, []);

    const onProjectPointerMove = useCallback(
        (event: PointerEvent) => {
            const drag = projectDragRef.current;
            if (!drag) return;
            if (!drag.active) {
                if (Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < 5) return;
                drag.active = true;
                drag.sequence = ++projectDragSequenceRef.current;
                projectGhostPointRef.current = { x: event.clientX, y: event.clientY };
                setProjectDragVisual({
                    width: drag.width,
                    height: drag.height,
                    grabX: drag.grabX,
                    grabY: drag.grabY,
                    row: cloneProjectRow(drag.sourceRow),
                });
                setDraggingProjectId(drag.sourceId);
                document.body.classList.add("is-sorting-projects");
            }
            event.preventDefault();
            moveProjectGhost(drag, event.clientX, event.clientY);
            const drop = resolveProjectDrop(event.clientX, event.clientY);
            setProjectDrop((current) => (current?.targetId === drop?.targetId && current?.placement === drop?.placement ? current : drop));
            if (drop) animateProjectOrder(drag.sourceId, drop);
        },
        [animateProjectOrder, moveProjectGhost, resolveProjectDrop],
    );

    const onProjectPointerUp = useCallback(
        (event: PointerEvent) => {
            const drag = projectDragRef.current;
            const drop = drag?.active ? resolveProjectDrop(event.clientX, event.clientY) : null;
            const dragged = !!drag?.active;
            if (drag && drop) animateProjectOrder(drag.sourceId, drop);
            endProjectDrag(!dragged);
            if (dragged && drag) settleProjectGhost(drag);
            suppressProjectClickRef.current = dragged;
            if (dragged) window.setTimeout(() => (suppressProjectClickRef.current = false), 0);
        },
        [animateProjectOrder, endProjectDrag, resolveProjectDrop, settleProjectGhost],
    );

    const onProjectPointerDown = (event: ReactPointerEvent<HTMLButtonElement>, sourceId: string) => {
        if (event.button !== 0) return;
        if (!sessionsById[sourceId]) return;
        const bounds = event.currentTarget.getBoundingClientRect();
        projectDragRef.current = {
            sourceId,
            startX: event.clientX,
            startY: event.clientY,
            grabX: Math.min(Math.max(event.clientX - bounds.left, 18), Math.max(bounds.width - 18, 18)),
            grabY: Math.min(Math.max(event.clientY - bounds.top, 8), Math.max(bounds.height - 8, 8)),
            width: bounds.width,
            height: bounds.height,
            sourceRow: event.currentTarget,
            active: false,
            sequence: 0,
        };
        projectMoveHandlerRef.current = onProjectPointerMove;
        projectUpHandlerRef.current = onProjectPointerUp;
        window.addEventListener("pointermove", onProjectPointerMove);
        window.addEventListener("pointerup", onProjectPointerUp, { once: true });
    };

    const selectProject = (id: string) => {
        if (suppressProjectClickRef.current) {
            suppressProjectClickRef.current = false;
            return;
        }
        cmd.selectSession(id);
    };

    const projectDragClass = (id: string) => {
        if (draggingProjectId === id) return " project-drag-source";
        if (projectDrop?.targetId === id) return ` project-drop-${projectDrop.placement}`;
        return "";
    };

    useEffect(() => () => endProjectDrag(), [endProjectDrag]);

    useLayoutEffect(() => {
        if (!projectGhostRef.current || !projectDragVisual) return;
        projectGhostRef.current.replaceChildren(projectDragVisual.row);
    }, [projectDragVisual]);

    const jumpToWindow = (sessionId: string, winId: string) => {
        if (sessionId !== activeSessionId) cmd.selectSession(sessionId);
        cmd.selectWindowId(winId);
    };
    const jumpToAgents = (sessionId: string) => {
        if (sessionId !== activeSessionId) cmd.selectSession(sessionId);
        cmd.focusAgents();
    };

    const SessionCloseButton = ({ session }: { session: Session }) => (
        <Tooltip label={`Close ${session.name}`}>
            <button type="button" className="sess-close" aria-label={`Close ${session.name}`} onClick={() => cmd.closeSession(session.id)}>
                <IconClose size={11} />
            </button>
        </Tooltip>
    );

    const ProjectBlock = ({ s }: { s: Session }) => {
        const active = s.id === activeSessionId;
        const winIds = windowsBySession[s.id] ?? [];
        const sessionWindows = winIds.map((id) => windowsById[id]).filter(Boolean) as Window[];
        const agentIds = agentsBySession[s.id] ?? [];
        const agents = agentIds.map((id) => agentsById[id]).filter(Boolean);
        const rollup = rollupAgentStates(agents.map((agent) => activityById[agent.id]));
        const tabCount = sessionWindows.filter((w) => w.role === "term").length;

        if (!active) {
            const visible = agents.slice(0, MAX_BADGE_ICONS);
            const overflow = agents.length - visible.length;
            return (
                <div className={`session-row-shell project-row-shell${projectDragClass(s.id)}`} data-project-id={s.id}>
                    <Tooltip label={s.cwd || s.name} side="right">
                        <button
                            className="proj-row collapsed"
                            data-project-drop-row
                            aria-grabbed={draggingProjectId === s.id}
                            onPointerDown={(event) => onProjectPointerDown(event, s.id)}
                            onClick={() => selectProject(s.id)}>
                            <span className="proj-folder">
                                <IconFolder size={12} />
                            </span>
                            <span className="proj-name">{s.name}</span>
                            {visible.length > 0 && (
                                <span className="proj-child-icons">
                                    {visible.map((a) => (
                                        <span
                                            key={a.id}
                                            className={`proj-pip proj-pip-${a.type}${activityById[a.id] ? ` state-${activityById[a.id].state}` : ""}`}>
                                            <AgentIcon type={a.type} size={20} />
                                        </span>
                                    ))}
                                    {overflow > 0 && <span className="proj-child-icons-more">+{overflow}</span>}
                                </span>
                            )}
                            {rollup && <AgentStateIndicator state={rollup} />}
                        </button>
                    </Tooltip>
                    <SessionCloseButton session={s} />
                </div>
            );
        }

        const winByRole = (role: WindowRole): Window | undefined => sessionWindows.find((w) => w.role === role);
        const activeRole = sessionWindows.find((w) => w.id === s.activeWindowId)?.role;
        const inAgentView = s.view === "agent";
        const inWindowsView = s.view === "windows";
        const isSubActive = (role: WindowRole | "agents"): boolean => {
            if (role === "agents") return inAgentView;
            return inWindowsView && activeRole === role;
        };

        const onSubClick = (role: WindowRole | "agents") => {
            if (role === "agents") {
                jumpToAgents(s.id);
                return;
            }
            const w = winByRole(role);
            if (w) {
                jumpToWindow(s.id, w.id);
                return;
            }
            if (s.id !== activeSessionId) cmd.selectSession(s.id);
            if (role === "term") cmd.newWindow();
            else if (role === "search") cmd.focusGlobalSearch();
        };

        const termIcons: React.ReactNode[] =
            tabCount > 1
                ? Array.from({ length: tabCount }, (_, i) => (
                      <span key={i} className="proj-pip proj-pip-term">
                          <IconCommand size={14} />
                      </span>
                  ))
                : [];
        const agentIcons: React.ReactNode[] = agents.map((a) => (
            <span key={a.id} className={`proj-pip proj-pip-${a.type}${activityById[a.id] ? ` state-${activityById[a.id].state}` : ""}`}>
                <AgentIcon type={a.type} size={20} />
            </span>
        ));

        type SubRow = {
            role: WindowRole | "agents";
            label: string;
            kbd?: string;
            title: string;
            icons: React.ReactNode[];
        };
        const children: SubRow[] = [
            { role: "files", label: "Files", kbd: kb("window.files"), title: `Files — ${kb("window.files")}`, icons: [] },
            {
                role: "term",
                label: "Term",
                kbd: kb("window.terminal"),
                title: `Term${tabCount > 1 ? ` · ${tabCount} tabs` : ""} — ${kb("window.terminal")}`,
                icons: termIcons,
            },
            { role: "diff", label: "Diff", kbd: kb("window.git"), title: `Diff — ${kb("window.git")}`, icons: [] },
            {
                role: "agents",
                label: "Agents",
                kbd: kb("window.agents"),
                title: `Agents${agents.length ? ` · ${agents.length}` : ""} — ${kb("window.agents")}`,
                icons: agentIcons,
            },
            { role: "search", label: "Search", kbd: kb("window.search"), title: `Search — ${kb("window.search")}`, icons: [] },
        ];
        return (
            <div className={`proj-tree active${projectDragClass(s.id)}`} data-project-id={s.id}>
                <div className="session-row-shell project-row-shell">
                    <Tooltip label={s.cwd || s.name} side="right">
                        <button
                            className="proj-row expanded"
                            data-project-drop-row
                            aria-grabbed={draggingProjectId === s.id}
                            onPointerDown={(event) => onProjectPointerDown(event, s.id)}
                            onClick={() => selectProject(s.id)}>
                            <span className="proj-folder">
                                <IconFolder size={12} />
                            </span>
                            <span className="proj-name">{s.name}</span>
                        </button>
                    </Tooltip>
                    <SessionCloseButton session={s} />
                </div>
                <div className="proj-children">
                    {children.map((c) => {
                        const subActive = isSubActive(c.role);
                        const node = c.role === "agents" ? <IconAgent size={13} /> : <WindowIcon role={c.role} size={13} />;
                        const visibleIcons = c.icons.slice(0, MAX_BADGE_ICONS);
                        const overflow = c.icons.length - visibleIcons.length;
                        return (
                            <Tooltip key={c.role} label={c.title} side="right">
                                <button
                                    type="button"
                                    className={`proj-child${subActive ? " active" : ""}`}
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        onSubClick(c.role);
                                    }}>
                                    <span className="proj-child-tick" />
                                    <span className="proj-child-ic">{node}</span>
                                    <span className="proj-child-label">{c.label}</span>
                                    {visibleIcons.length > 0 && (
                                        <span className="proj-child-icons">
                                            {visibleIcons}
                                            {overflow > 0 && <span className="proj-child-icons-more">+{overflow}</span>}
                                        </span>
                                    )}
                                    {c.role === "agents" && rollup && <AgentStateIndicator state={rollup} />}
                                    {c.kbd && <span className="proj-child-kbd">{c.kbd}</span>}
                                </button>
                            </Tooltip>
                        );
                    })}
                </div>
            </div>
        );
    };

    const SimpleRow = ({ s }: { s: Session }) => {
        const active = s.id === activeSessionId;
        return (
            <div className="session-row-shell">
                <button className={`sess-row${active ? " active" : ""}`} onClick={() => cmd.selectSession(s.id)}>
                    <span className={`sess-icon ${s.kind}`}>
                        <span className="sess-icon-glyph">{kindIcon(s.kind)}</span>
                    </span>
                    <span className="sess-name">{s.name}</span>
                </button>
                <SessionCloseButton session={s} />
            </div>
        );
    };

    const renderSession = (s: Session) => (s.kind === "project" ? <ProjectBlock key={s.id} s={s} /> : <SimpleRow key={s.id} s={s} />);

    const Group = ({
        label,
        list,
        add,
        addTitle,
        addKbd,
        action,
        actionTitle,
        emptyText,
    }: {
        label: string;
        list: Session[];
        add?: () => void;
        addTitle?: string;
        addKbd?: string;
        action?: () => void;
        actionTitle?: string;
        emptyText: string;
    }) => (
        <Panel variant="group">
            <PanelHeader
                label={label}
                rule
                extra={
                    add && (
                        <span className="rail-group-actions">
                            {addKbd && <span className="rail-group-kbd">{addKbd}</span>}
                            {action && (
                                <Tooltip label={actionTitle}>
                                    <button className="rail-group-add" onClick={action} aria-label={actionTitle} type="button">
                                        <IconPencil size={11} />
                                    </button>
                                </Tooltip>
                            )}
                            <Tooltip label={addTitle}>
                                <button className="rail-group-add" onClick={add} aria-label={addTitle} type="button">
                                    <IconPlus size={11} />
                                </button>
                            </Tooltip>
                        </span>
                    )
                }
            />
            {list.length === 0 ? (
                <EmptyState variant="inline" message={emptyText} action={add ? { label: emptyText, onClick: add } : undefined} />
            ) : (
                list.map(renderSession)
            )}
        </Panel>
    );

    const ghostPoint = projectGhostPointRef.current;
    const ghostTransform =
        projectDragVisual && ghostPoint
            ? `translate3d(${ghostPoint.x - projectDragVisual.grabX}px, ${ghostPoint.y - projectDragVisual.grabY}px, 0)`
            : "translate3d(-100vw, -100vh, 0)";

    return (
        <>
            <aside className="side-rail">
                <div className="rail-scroll">
                    <Group
                        label="Projects"
                        list={projects}
                        add={() => cmd.openPicker("projects")}
                        addTitle={`Open project — ${kb("project.open")}`}
                        addKbd={kb("project.open")}
                        emptyText="no projects"
                    />
                    <Group
                        label="SSH"
                        list={sshs}
                        add={() => cmd.openPicker("ssh")}
                        addTitle={`Connect to SSH host — ${kb("ssh.open")}`}
                        addKbd={kb("ssh.open")}
                        action={() => void cmd.openSshConfigEditor()}
                        actionTitle="Edit ~/.ssh/config"
                        emptyText="no ssh hosts"
                    />
                    <Group
                        label="Cloud"
                        list={cloud}
                        add={cmd.openAwsSession}
                        addTitle={`Open AWS — ${kb("aws.open")}`}
                        addKbd={kb("aws.open")}
                        emptyText="no cloud sessions"
                    />
                    <Group
                        label="CI/CD"
                        list={cicd}
                        add={cmd.openRundeckSession}
                        addTitle="Open Rundeck deploy center"
                        emptyText="open rundeck deploy center"
                    />
                    <Group
                        label="API"
                        list={apis}
                        add={() => cmd.openPicker("bruno")}
                        addTitle={`Open Bruno workspace — ${kb("bruno.open")}`}
                        addKbd={kb("bruno.open")}
                        emptyText="open a bruno workspace"
                    />
                    <Group label="Command" list={commands} add={cmd.createCommandSession} addTitle="New command session" emptyText="no commands" />
                </div>

                <UpdateChip />

                {/* Identity lives at the foot of the rail: present when you look for
                it, out of the way of the sessions above it. */}
                <div className="rail-sig">
                    <Logo size={13} />
                    <span className="rail-sig-name">Sikemux</span>
                    <VersionChip />
                </div>
            </aside>
            {projectDragVisual &&
                createPortal(
                    <div
                        ref={projectGhostRef}
                        className="project-drag-ghost"
                        data-project-drag-ghost
                        aria-hidden="true"
                        style={{ width: projectDragVisual.width, height: projectDragVisual.height, transform: ghostTransform }}
                    />,
                    document.body,
                )}
        </>
    );
}
