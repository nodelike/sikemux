import {
    createContext,
    memo,
    useCallback,
    useContext,
    useEffect,
    useLayoutEffect,
    useMemo,
    useRef,
    useState,
    type MouseEvent as ReactMouseEvent,
    type PointerEvent as ReactPointerEvent,
    type ReactNode,
    type RefObject,
} from "react";
import { createPortal, flushSync } from "react-dom";
import { keybindingLabelForAction, type KeybindingActionId, keybindingOverridesOf } from "../commands/keybindings";
import type { Agent, Session, SessionKind, Window } from "../state/types";
import * as cmd from "../state/commands";
import { animate, EASE_IN, EASE_PAN, EASE_POP, EASE_SWAP, foldedFrames, leavingRef, prefersReducedMotion } from "../lib/motion";
import { getState, useStore } from "../state/store";
import { useShallow } from "zustand/react/shallow";
import { AgentFace } from "../buddies/Buddy";
import { IconAgent, IconClose, IconFolder, IconMove, IconPencil, IconPlus, IconServer, IconSettings, IconTerminal, WindowIcon } from "../ui/Icons";
import { useHostRepo } from "../codehost/project";
import { codeHost } from "../codehost/registry";
import { isTreeSection } from "../codehost/state";
import { SectionIcon } from "../codehost/components/ActionsIcon";
import { sectionLabel, sectionsOf } from "../codehost/sections";
import { collectPanes } from "../state/layout";
import { Tooltip } from "../ui/Tooltip";
import { EmptyState, Panel, PanelHeader } from "../ui/Panel";
import { RailMasthead } from "./RailMasthead";
import { RailAccount } from "../account/RailAccount";
import { UpdateChip } from "./UpdateChip";
import { AgentStateIndicator, showsAgentState } from "../agents/AgentStateIndicator";
import { agentIdsOf } from "../state/selectors";
import { PROJECT_SURFACE_ORDER, PROJECT_SURFACES, type ProjectSurfaceRole } from "../workbench/surfaces";
import { pluginSurface, type FrontendPlugin } from "../plugins/registry";
import type { PluginSignIn } from "../plugin-api/signInState";
import { StatusDot, type StatusTone } from "../ui/StatusDot";
import { useSessionName } from "../terminal/railNames";
import { useSshTone } from "../terminal/sshLinks";
import { useInstalledPlugins } from "../plugins/installed";
import { railGroupOf, type RailGroup } from "../state/railGroups";
import { isPluginKind, pluginIdOf } from "../plugins/kinds";
import { leavingRail } from "./railMotion";
import { LEAVES_SETTINGS } from "../settings/leaveSettings";
import { TreeContextMenu, type CtxItem } from "./FileTree";
import { newSpace, SpaceGlyph, SpaceSwitch } from "./SpaceSwitch";
import { shownProjects } from "../state/projectSpaces";
import type { ProjectSpace } from "../state/types";
import { HostRows } from "./HostsGroup";
import { openOnHost, type HostEntry } from "../hosts/hosts";
import { reportError } from "../state/toast";

/* Line icons keep a margin inside their box and logos run edge to edge, so each is drawn at the size that inks the same area. */
const LINE_ICON_SIZE = 13;
const LOGO_SIZE = 13;

function kindIcon(kind: SessionKind): ReactNode {
    if (kind === "project") return <IconFolder size={LINE_ICON_SIZE} />;
    if (kind === "ssh") return <IconServer size={LINE_ICON_SIZE} />;
    const surface = pluginSurface(kind);
    if (surface) return surface.icon(LOGO_SIZE);
    return <IconTerminal size={LINE_ICON_SIZE} />;
}

const MAX_BADGE_ICONS = 5;
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

/*
 * The row under the pointer, lifted out of the rail into the ghost.
 *
 * It used to arrive carrying a copy of every computed style of every element
 * inside it — hundreds of inline properties read one at a time, at the moment
 * the drag starts. It does not need any of them: everything a project row
 * paints is styled by class and nothing is scoped to the rail, so the clone
 * looks the same wherever it lands.
 */
function cloneProjectRow(source: HTMLElement): HTMLElement {
    const clone = source.cloneNode(true) as HTMLElement;
    clone.classList.add("project-drag-ghost-row");
    clone.removeAttribute("data-project-drop-row");
    clone.removeAttribute("aria-grabbed");
    clone.setAttribute("tabindex", "-1");
    return clone;
}

function projectElement(id: string): HTMLElement | null {
    return document.querySelector<HTMLElement>(`[data-project-id="${CSS.escape(id)}"]`);
}

function projectRects(): Map<string, DOMRect> {
    return new Map(
        Array.from(document.querySelectorAll<HTMLElement>("[data-project-id]"), (element) => [
            element.dataset.projectId ?? "",
            element.getBoundingClientRect(),
        ]),
    );
}

/*
 * What the rows below need from the rail they are in.
 *
 * These four used to be declared inside the rail's own render body, which made
 * them a different component type on every render: React tore the whole rail
 * out of the tree and rebuilt it whenever anything changed. Tooltips
 * disappeared mid-hover, focus was lost, and a drag in progress was holding
 * refs to rows that no longer existed.
 */
interface RailContextValue {
    activeSessionId: string;
    draggingProjectId: string | null;
    projectDragClass: (id: string) => string;
    onProjectPointerDown: (event: ReactPointerEvent<HTMLButtonElement>, sourceId: string) => void;
    selectProject: (id: string) => void;
    openSpaceMenu: (event: ReactMouseEvent, session: Session) => void;
    jumpToWindow: (sessionId: string, winId: string) => void;
    kb: (id: KeybindingActionId) => string;
}

const RailContext = createContext<RailContextValue | null>(null);

function useRail(): RailContextValue {
    const value = useContext(RailContext);
    if (!value) throw new Error("side rail rows must render inside the rail");
    return value;
}

function SessionCloseButton({ session }: { session: Session }) {
    const name = useSessionName(session);
    return (
        <Tooltip label={`Close ${name}`}>
            <button type="button" className="row-x" aria-label={`Close ${name}`} onClick={() => cmd.closeSession(session.id)}>
                <IconClose size={11} />
            </button>
        </Tooltip>
    );
}

function SimpleRow({ s, status }: { s: Session; status?: ReactNode }) {
    const { activeSessionId } = useRail();
    const active = s.id === activeSessionId;
    const name = useSessionName(s);
    return (
        <div className="session-row-shell">
            <button className={`sess-row${active ? " active" : ""}`} onClick={() => cmd.selectSession(s.id)}>
                <span className={`sess-icon ${s.kind}`}>
                    <span className="sess-icon-glyph">{kindIcon(s.kind)}</span>
                </span>
                <span className="sess-name">{name}</span>
            </button>
            {status && <span className="row-status">{status}</span>}
            <SessionCloseButton session={s} />
        </div>
    );
}

/* One project's tree folds while the next opens, on the same curve, so what sits below them does not move. */
const PROJECT_SWAP_MS = 200;

const foldChildren = leavingRef<HTMLDivElement>((children) => {
    const [open, closed] = foldedFrames(children);
    children.style.overflow = "hidden";
    animate(children, [{ opacity: 1 }, { opacity: 0 }], { duration: 120, easing: EASE_IN, fill: "forwards" });
    return animate(children, [open, closed], { duration: PROJECT_SWAP_MS, easing: EASE_SWAP });
});

function useProjectUnfold(tree: RefObject<HTMLDivElement | null>, active: boolean): void {
    const was = useRef(active);
    const openPadding = useRef("0px");
    useLayoutEffect(() => {
        const el = tree.current;
        if (!el) return;
        const before = was.current;
        was.current = active;
        if (active) openPadding.current = getComputedStyle(el).paddingBottom;
        if (before === active) return;
        const options = { duration: PROJECT_SWAP_MS, easing: EASE_SWAP };
        const padding = openPadding.current;
        if (!active) {
            animate(el, [{ paddingBottom: padding }, { paddingBottom: "0px" }], options);
            return;
        }
        animate(el, [{ paddingBottom: "0px" }, { paddingBottom: padding }], options);
        const children = el.querySelector<HTMLElement>(":scope > .proj-children");
        if (!children) return;
        const [open, closed] = foldedFrames(children);
        children.style.overflow = "hidden";
        const run = animate(children, [closed, open], options);
        const settle = () => (children.style.overflow = "");
        if (run) run.finished.then(settle, settle);
        else settle();
        [...children.children].forEach((child, i) =>
            animate(
                child,
                [
                    { opacity: 0, transform: "translateY(-3px)" },
                    { opacity: 1, transform: "none" },
                ],
                {
                    duration: 170,
                    delay: 40 + i * 16,
                    fill: "backwards",
                },
            ),
        );
    }, [active, tree]);
}

/* Each project reads only its own windows, agents and their states, so one agent changing state re-renders its own project and not the rail. */
const ProjectBlock = memo(function ProjectBlock({ s }: { s: Session }) {
    const rail = useRail();
    const { activeSessionId, draggingProjectId, kb } = rail;
    const active = s.id === activeSessionId;
    // Active or not, a project is one tree root holding its row, so the row survives the switch and its tint cross-fades.
    const treeRef = useRef<HTMLDivElement>(null);
    useProjectUnfold(treeRef, active);
    const host = useHostRepo(active ? s.cwd : null, active).repo;
    const hostDef = host ? codeHost(host.provider) : undefined;
    const gitArea = useStore((st) => {
        for (const id of st.windowsBySession[s.id] ?? []) {
            const win = st.windows[id];
            const pane = win?.role === "git" ? collectPanes(win.root).find((candidate) => candidate.kind === "git") : undefined;
            if (pane) return st.gitViews[pane.id]?.area ?? "local";
        }
        return null;
    });
    const sessionWindows = useStore(
        useShallow((st) => (st.windowsBySession[s.id] ?? []).flatMap((id): Window[] => (st.windows[id] ? [st.windows[id]] : []))),
    );
    const agents = useStore(useShallow((st) => agentIdsOf(st, s.id).flatMap((id): Agent[] => (st.agents[id] ? [st.agents[id]] : []))));
    const rollup = useStore((st) => st.projectStatus[s.id]);

    const visibleAgents = agents.slice(0, MAX_BADGE_ICONS);
    const agentOverflow = agents.length - visibleAgents.length;
    const agentBadge = (
        <>
            {visibleAgents.length > 0 && (
                <span className="proj-child-icons">
                    {visibleAgents.map((a) => (
                        <AgentFace
                            key={a.id}
                            agent={a}
                            size={16}
                            logoSize={20}
                            logoClass={`proj-pip proj-pip-${a.type}`}
                            buddyClass="proj-pip buddy-mark"
                        />
                    ))}
                    {agentOverflow > 0 && <span className="proj-child-icons-more">+{agentOverflow}</span>}
                </span>
            )}
            {showsAgentState(rollup) && (
                <span className="proj-row-status">
                    <AgentStateIndicator display={rollup} />
                </span>
            )}
        </>
    );

    if (!active) {
        return (
            <div ref={treeRef} className={`proj-tree${rail.projectDragClass(s.id)}`} data-project-id={s.id}>
                <div className="session-row-shell project-row-shell" onContextMenu={(event) => rail.openSpaceMenu(event, s)}>
                    <Tooltip label={s.cwd || s.name} side="right">
                        <button
                            className="proj-row collapsed"
                            data-project-drop-row
                            aria-grabbed={draggingProjectId === s.id}
                            onPointerDown={(event) => rail.onProjectPointerDown(event, s.id)}
                            onClick={() => rail.selectProject(s.id)}>
                            <span className="proj-folder">
                                <IconFolder size={LINE_ICON_SIZE} />
                            </span>
                            <span className="proj-name">{s.name}</span>
                            {agentBadge}
                        </button>
                    </Tooltip>
                    <SessionCloseButton session={s} />
                </div>
            </div>
        );
    }

    const activeRole = sessionWindows.find((w) => w.id === s.activeWindowId)?.role;
    const tabCount = sessionWindows.filter((w) => w.role === "term").length;
    const termIcons: ReactNode[] =
        tabCount > 1
            ? Array.from({ length: tabCount }, (_, i) => (
                  <span key={i} className="proj-pip proj-pip-term">
                      <IconTerminal size={LINE_ICON_SIZE} />
                  </span>
              ))
            : [];
    const surfaceRow = (role: ProjectSurfaceRole): SubRow => {
        const { label, shortcut, open } = PROJECT_SURFACES[role];
        return {
            key: role,
            icon: <WindowIcon role={role} size={13} />,
            label,
            kbd: kb(shortcut),
            title: `${label} — ${kb(shortcut)}`,
            open: role === "git" && gitArea && isTreeSection(gitArea) ? () => cmd.openGitArea("local") : open,
            active: activeRole === role && !(role === "git" && gitArea && isTreeSection(gitArea)),
            icons: [],
        };
    };
    const hostRows: SubRow[] = hostDef
        ? sectionsOf(hostDef)
              .filter(isTreeSection)
              .map((section) => ({
                  key: section,
                  icon: <SectionIcon section={section} size={13} />,
                  label: sectionLabel(hostDef, section),
                  title: sectionLabel(hostDef, section),
                  open: () => cmd.openGitArea(section),
                  active: activeRole === "git" && gitArea === section,
                  icons: [],
              }))
        : [];
    const terminalRow: SubRow = {
        key: "term",
        icon: <WindowIcon role="term" size={13} />,
        label: "Terminal",
        kbd: kb("window.terminal"),
        title: `Terminal${tabCount > 1 ? ` · ${tabCount} tabs` : ""} — ${kb("window.terminal")}`,
        open: () => cmd.selectWindowByRole("term"),
        active: activeRole === "term",
        icons: termIcons,
    };
    const agentsRow: SubRow = {
        key: "agents",
        icon: <IconAgent size={13} />,
        label: "Agents",
        kbd: kb("window.agents"),
        title: `Agents${agents.length ? ` · ${agents.length}` : ""} — ${kb("window.agents")}`,
        open: () => cmd.focusAgents(),
        active: activeRole === "agent",
        icons: [],
    };
    const children: SubRow[] = PROJECT_SURFACE_ORDER.flatMap((role) => {
        const row = surfaceRow(role);
        if (role === "files") return [row, terminalRow];
        if (role === "git") return [agentsRow, row, ...hostRows];
        return [row];
    });
    return (
        <div ref={treeRef} className={`proj-tree active${rail.projectDragClass(s.id)}`} data-project-id={s.id}>
            <div className="session-row-shell project-row-shell" onContextMenu={(event) => rail.openSpaceMenu(event, s)}>
                <Tooltip label={s.cwd || s.name} side="right">
                    <button
                        className="proj-row expanded"
                        data-project-drop-row
                        aria-grabbed={draggingProjectId === s.id}
                        onPointerDown={(event) => rail.onProjectPointerDown(event, s.id)}
                        onClick={() => rail.selectProject(s.id)}>
                        <span className="proj-folder">
                            <IconFolder size={LINE_ICON_SIZE} />
                        </span>
                        <span className="proj-name">{s.name}</span>
                        {agentBadge}
                    </button>
                </Tooltip>
                <SessionCloseButton session={s} />
            </div>
            <div className="proj-children" ref={foldChildren}>
                {children.map((c) => {
                    return (
                        <Tooltip key={c.key} label={c.title} side="right">
                            <button
                                type="button"
                                className={`proj-child${c.active ? " active" : ""}`}
                                aria-label={c.label}
                                aria-current={c.active ? "page" : undefined}
                                onClick={(e) => {
                                    e.stopPropagation();
                                    c.open();
                                }}>
                                <span className="proj-child-tick" />
                                <span className="proj-child-ic">{c.icon}</span>
                                <span className="proj-child-label">{c.label}</span>
                                {c.icons.length > 0 && <FittedIcons icons={c.icons} />}
                                {c.kbd && <span className="proj-child-kbd">{c.kbd}</span>}
                            </button>
                        </Tooltip>
                    );
                })}
            </div>
        </div>
    );
});

interface SubRow {
    key: string;
    icon: ReactNode;
    label: string;
    kbd?: string;
    title: string;
    open: () => void;
    active: boolean;
    icons: ReactNode[];
}

function renderSession(s: Session) {
    if (s.kind === "project") return <ProjectBlock key={s.id} s={s} />;
    return <SimpleRow key={s.id} s={s} status={s.kind === "ssh" ? <SshDot sessionId={s.id} /> : undefined} />;
}

const SSH_TONE_LABEL: Record<StatusTone, string> = { ok: "Connected", pending: "Connecting", idle: "Not connected", failed: "Connection failed" };

function SshDot({ sessionId }: { sessionId: string }) {
    const tone = useSshTone(sessionId);
    return <StatusDot tone={tone} label={SSH_TONE_LABEL[tone]} />;
}

const SIGN_IN_TONE: Record<PluginSignIn, { tone: StatusTone; label: string }> = {
    "signed-in": { tone: "ok", label: "Signed in" },
    checking: { tone: "pending", label: "Checking sign-in" },
    "signed-out": { tone: "idle", label: "Not signed in" },
    failed: { tone: "failed", label: "Sign-in failed" },
};

function SignInDot({ useSignIn }: { useSignIn: () => PluginSignIn }) {
    const { tone, label } = SIGN_IN_TONE[useSignIn()];
    return <StatusDot tone={tone} label={label} />;
}

function signInDot(plugin: FrontendPlugin): ReactNode {
    return plugin.useSignIn ? <SignInDot useSignIn={plugin.useSignIn} /> : undefined;
}

/** A plugin that is enabled but not open yet: the same row its session will be, opening it on click. */
function PluginLauncherRow({ plugin, name }: { plugin: FrontendPlugin; name: string }) {
    const surface = plugin.surfaces[0];
    if (!surface) return null;
    const status = signInDot(plugin);
    return (
        <div className="session-row-shell">
            <button className="sess-row" onClick={plugin.open}>
                <span className={`sess-icon ${surface.kind}`}>
                    <span className="sess-icon-glyph">{kindIcon(surface.kind)}</span>
                </span>
                <span className="sess-name">{name}</span>
            </button>
            {status && <span className="row-status">{status}</span>}
        </div>
    );
}

type OpenMenu = (event: ReactMouseEvent, items: CtxItem[]) => void;

function projectSpaceItems(session: Session, spaces: readonly ProjectSpace[], current: string | undefined, hosts: readonly HostEntry[]): CtxItem[] {
    const mark = (selected: boolean) => (selected ? "✓" : undefined);
    const elsewhere = [
        ...(session.host ? [{ id: undefined, name: "This host", reach: "connected" }] : []),
        ...hosts.filter((host) => host.id !== session.host),
    ];
    return [
        { label: "Project settings", icon: <IconSettings size={13} />, run: () => cmd.openProjectSettings(session.id) },
        ...elsewhere.map((host) => ({
            label: `Open on ${host.name}`,
            icon: <IconServer size={13} />,
            disabled: host.reach !== "connected",
            run: () => void openOnHost(session, host).catch(reportError(`Open ${session.name} on ${host.name}`)),
        })),
        { sep: true },
        ...spaces.map((space) => ({
            label: space.name,
            icon: <SpaceGlyph icon={space.icon} />,
            hint: mark(current === space.id),
            run: () => cmd.setProjectSpace(session.cwd, space.id),
        })),
        { label: "New Space…", icon: <IconPlus size={13} />, run: () => void newSpace((id) => cmd.setProjectSpace(session.cwd, id)) },
        { sep: true },
        { label: "No Space", icon: <IconMove size={13} />, hint: mark(current === undefined), run: () => cmd.setProjectSpace(session.cwd, null) },
    ];
}

function Group({
    label,
    list,
    rows,
    add,
    addTitle,
    addKbd,
    action,
    actionTitle,
    emptyText,
    singleton,
    className,
    toolbar,
}: {
    label: string;
    list: Session[];
    add?: () => void;
    addTitle?: string;
    addKbd?: string;
    action?: () => void;
    actionTitle?: string;
    emptyText: string;
    singleton?: boolean;
    /** Rows drawn in place of `list`, for a group that is more than its sessions. */
    rows?: ReactNode;
    className?: string;
    toolbar?: ReactNode;
}) {
    return (
        <Panel variant="group" className={className}>
            <PanelHeader
                label={label}
                rule
                extra={
                    add &&
                    !singleton && (
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
            {toolbar}
            {rows ??
                (list.length === 0 ? (
                    <EmptyState variant="inline" message={emptyText} action={add ? { label: emptyText, onClick: add } : undefined} />
                ) : (
                    list.map(renderSession)
                ))}
        </Panel>
    );
}

export const SideRail = memo(function SideRail() {
    const sessionsById = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const rawActiveSessionId = useStore((s) => s.activeSessionId);
    const settingsOpen = useStore((s) => s.settingsOpen);
    const keybindingOverrides = useStore(keybindingOverridesOf);
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

    const pluginManifests = useStore((s) => s.pluginManifests);
    const disabledPlugins = useStore((s) => s.disabledPlugins);
    const inGroup = (group: RailGroup) => sessions.filter((s) => railGroupOf(s.kind, pluginManifests, disabledPlugins) === group);
    const projectSpaces = useStore((s) => s.projectSpaces);
    const spaces = useStore((s) => s.spaces);
    const activeSpaceId = useStore((s) => s.activeSpaceId);
    const [menu, setMenu] = useState<{ x: number; y: number; items: CtxItem[] } | null>(null);
    const openMenu: OpenMenu = (event, items) => {
        event.preventDefault();
        setMenu({ x: event.clientX, y: event.clientY, items });
    };
    const chosenHost = useStore((s) => s.chosenHost);
    // Each host has its own projects (ADR 0019); an SSH host without Sikemux has none.
    const allProjects = inGroup("project").filter((session) => chosenHost.kind === "core" && session.host === chosenHost.host);
    const projects = shownProjects(allProjects, projectSpaces, activeSpaceId);
    const activeSpace = spaces.find((space) => space.id === activeSpaceId);
    const sshs = inGroup("ssh");
    const commands = inGroup("command").filter((session) => chosenHost.kind === "core" && session.host === chosenHost.host);
    const plugins = inGroup("plugins");
    const enabledPlugins = useInstalledPlugins();
    // Every enabled plugin always has a row; its session is only made on the first click.
    const pluginRows = enabledPlugins.flatMap((plugin) => {
        const opened = plugins.filter((session) => isPluginKind(session.kind) && pluginIdOf(session.kind) === plugin.id);
        if (opened.length > 0) return opened.map((session) => <SimpleRow key={session.id} s={session} status={signInDot(plugin)} />);
        return [<PluginLauncherRow key={plugin.id} plugin={plugin} name={plugin.manifest.name} />];
    });

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
        const before = prefersReducedMotion() ? null : projectRects();
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
                    easing: EASE_POP,
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
        if (!ghost || !destination || prefersReducedMotion() || typeof ghost.animate !== "function") {
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
            { duration: 180, easing: EASE_PAN, fill: "forwards" },
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

    useEffect(() => () => endProjectDrag(), [endProjectDrag]);

    useLayoutEffect(() => {
        if (!projectGhostRef.current || !projectDragVisual) return;
        projectGhostRef.current.replaceChildren(projectDragVisual.row);
    }, [projectDragVisual]);

    const jumpToWindow = (sessionId: string, winId: string) => {
        if (sessionId !== activeSessionId) cmd.selectSession(sessionId);
        cmd.selectWindowId(winId);
    };

    const ghostPoint = projectGhostPointRef.current;
    const ghostTransform =
        projectDragVisual && ghostPoint
            ? `translate3d(${ghostPoint.x - projectDragVisual.grabX}px, ${ghostPoint.y - projectDragVisual.grabY}px, 0)`
            : "translate3d(-100vw, -100vh, 0)";

    /* The handlers change on every render; the rows reach them through this, so the context changes only with what the rows show. */
    const handlers = useRef({ onProjectPointerDown, selectProject, openMenu, spaces, projectSpaces, jumpToWindow });
    handlers.current = { onProjectPointerDown, selectProject, openMenu, spaces, projectSpaces, jumpToWindow };
    const rail = useMemo<RailContextValue>(
        () => ({
            activeSessionId,
            draggingProjectId,
            projectDragClass: (id) => {
                if (draggingProjectId === id) return " project-drag-source";
                if (projectDrop?.targetId === id) return ` project-drop-${projectDrop.placement}`;
                return "";
            },
            onProjectPointerDown: (event, sourceId) => handlers.current.onProjectPointerDown(event, sourceId),
            selectProject: (id) => handlers.current.selectProject(id),
            openSpaceMenu: (event, session) => {
                const { openMenu, spaces, projectSpaces } = handlers.current;
                openMenu(event, projectSpaceItems(session, spaces, projectSpaces[session.cwd], getState().hosts));
            },
            jumpToWindow: (sessionId, winId) => handlers.current.jumpToWindow(sessionId, winId),
            kb: (id) => keybindingLabelForAction(keybindingOverrides, id),
        }),
        [activeSessionId, draggingProjectId, projectDrop, keybindingOverrides],
    );

    return (
        <RailContext.Provider value={rail}>
            <aside ref={leavingRail} className="side-rail" {...LEAVES_SETTINGS}>
                <RailMasthead />
                <div className="rail-scroll">
                    <Group
                        label="Projects"
                        list={projects}
                        add={() => cmd.openPicker("projects")}
                        addTitle={`Open project — ${kb("project.open")}`}
                        addKbd={kb("project.open")}
                        emptyText={activeSpace ? `no projects in ${activeSpace.name}` : "no projects"}
                        toolbar={allProjects.length > 0 && <SpaceSwitch spaces={spaces} activeSpaceId={activeSpaceId} />}
                    />
                    <Group
                        label="Hosts"
                        list={sshs}
                        rows={<HostRows onMenu={openMenu} />}
                        add={() => cmd.openPicker("ssh")}
                        addTitle={`Find a host — ${kb("ssh.open")}`}
                        addKbd={kb("ssh.open")}
                        action={() => void cmd.openSshConfigEditor()}
                        actionTitle="Edit ~/.ssh/config"
                        emptyText="no hosts"
                    />
                    <Group label="Plugins" list={plugins} rows={pluginRows} emptyText="no plugins" />
                    <Group label="Terminals" list={commands} add={cmd.createCommandSession} addTitle="New terminal" emptyText="no terminals" />
                </div>
                <UpdateChip />
                <RailAccount />
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
            {menu && <TreeContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}
        </RailContext.Provider>
    );
});

/* A mark is 20px wide and each one after it overlaps the last, by more for
   buddies, which fill their box, than for logos drawn small inside theirs. */
const MARK = 20;
const MARK_STEP = { buddies: 10, logos: 14 };
const MORE = 22;

/** As many marks as the row has room for, the rest counted as +N, so none is ever cut in half. */
function FittedIcons({ icons }: { icons: ReactNode[] }) {
    const ref = useRef<HTMLSpanElement>(null);
    const [room, setRoom] = useState(Infinity);
    const step = useStore((s) => (s.faceStyle.on ? MARK_STEP.buddies : MARK_STEP.logos));
    useLayoutEffect(() => {
        const el = ref.current;
        if (!el) return;
        const observer = new ResizeObserver(() => setRoom(el.clientWidth));
        observer.observe(el);
        return () => observer.disconnect();
    }, []);
    const width = (count: number, more: boolean) => (count > 0 ? MARK + (count - 1) * step : 0) + (more ? MORE : 0);
    let shown = Math.min(icons.length, MAX_BADGE_ICONS);
    while (shown > 0 && width(shown, shown < icons.length) > room) shown--;
    const overflow = icons.length - shown;
    return (
        <span ref={ref} className="proj-child-icons">
            {icons.slice(0, shown)}
            {overflow > 0 && <span className="proj-child-icons-more">+{overflow}</span>}
        </span>
    );
}
