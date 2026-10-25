import { useModalFocus } from "../hooks/useModalFocus";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { type AgentInfo, type AgentSession } from "../api/agents";
import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "./agentProfiles";
import { useMouseActive } from "../hooks/useMouseActive";
import { rankBy } from "@shared/lib/fuzzy";
import * as cmd from "../state/commands";
import { fetchResource, peekResource, useResource } from "../state/resources";
import { agentCatalogR, agentSessionsR } from "../state/resources.defs";
import { agentForChat, agentIdsOf, chatKey, hasChat } from "../state/selectors";
import { useStore } from "../state/store";
import type { Agent, AgentPermissionMode, AgentType, PinnedChat } from "../state/types";
import { AgentFace } from "../buddies/Buddy";
import { agentHandle } from "./AgentLabel";
import { AgentStateIndicator, showsAgentState } from "./AgentStateIndicator";
import { inCoreOrder, useInbox } from "../rail/useInbox";
import { AgentIcon, IconSearch, IconShield, IconShieldBolt } from "@shared/ui/Icons";
import { formatAge } from "@shared/lib/format";
import { agentAbout, agentLabel, canBypassPermissions, installCommand, loginArgs } from "@shared/agents/catalog";
import { installAgent, signInAgent } from "./AgentInstall";
import { withMissingAgents } from "./setupList";
import { leavingOverlay } from "../lib/motion";
import { swallow } from "../state/toast";

type Row = AgentSession & { type: AgentType };
/** A new chat, or with `signIn`, the agent's own sign-in for one that reads as signed out. */
type NewAgentItem = { kind: "new"; type: AgentType; signIn?: true };
/** A saved chat, with `agent` when it is open in the project. */
type ResumeAgentItem = { kind: "resume"; row: Row; agent?: Agent };
type AgentItem = NewAgentItem | ResumeAgentItem;

const NORMAL: AgentPermissionMode = "workspace-write";
const YOLO: AgentPermissionMode = "bypass";

const MODE_CHOICES: { mode: AgentPermissionMode; label: string; title: string }[] = [
    { mode: NORMAL, label: "safe", title: "Safe mode — the agent launches with normal approvals." },
    { mode: YOLO, label: "yolo", title: "YOLO mode — the agent launches without approvals." },
];

function sortRows(rows: Row[]): Row[] {
    return [...rows].sort((left, right) => right.mtime - left.mtime);
}

/** The project's chats as its rail lists them: open agents in the core's order, then pinned chats, then the rest newest first. */
function railOrder(rows: Row[], open: Agent[], pins: PinnedChat[]): ResumeAgentItem[] {
    const saved = new Map(rows.map((row) => [chatKey(row.type, row.id), row]));
    const used = new Set<string>();
    const take = (type: AgentType, id: string) => {
        used.add(chatKey(type, id));
        return saved.get(chatKey(type, id));
    };
    const isPinned = (agent: Agent) => pins.some((pin) => hasChat(agent, pin));
    const opened = (agent: Agent): ResumeAgentItem => {
        const id = agent.resumeId ?? agent.id;
        return { kind: "resume", row: take(agent.type, id) ?? { id, title: agent.title, mtime: 0, type: agent.type }, agent };
    };
    const live = open.filter((agent) => !isPinned(agent)).map(opened);
    const pinned = pins.flatMap((pin): ResumeAgentItem[] => {
        const agent = agentForChat(open, pin);
        if (agent) return [opened(agent)];
        const row = take(pin.agent, pin.id);
        return row ? [{ kind: "resume", row }] : [];
    });
    const rest = rows.filter((row) => !used.has(chatKey(row.type, row.id))).map((row): ResumeAgentItem => ({ kind: "resume", row }));
    return [...live, ...pinned, ...rest];
}

function typeForItem(item: AgentItem): AgentType {
    return item.kind === "new" ? item.type : item.row.type;
}

export function AgentPalette() {
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const session = useStore((state) => state.sessions[state.activeSessionId]);
    const windowsBySession = useStore((state) => state.windowsBySession);
    const windows = useStore((state) => state.windows);
    const agentsById = useStore((state) => state.agents);
    const pinnedChats = useStore((state) => state.pinnedChats);
    const agentStatus = useStore((state) => state.agentStatus);
    const { inbox } = useInbox();
    const profiles = useStore((state) => state.providerProfiles);
    const profileSelections = useStore((state) => state.selectedProviderProfileIds);
    const defaultMode = useStore((state) => state.defaultAgentPermissionMode);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    const found = useMemo(() => catalog.data ?? [], [catalog.data]);
    const agents = useMemo(() => (catalog.data ? withMissingAgents(catalog.data) : []), [catalog.data]);
    const origin = useRef({ sessionId: session?.id ?? "", cwd: session?.cwd ?? "" });
    const inputRef = useRef<HTMLInputElement>(null);
    const mouseActive = useMouseActive();
    const [query, setQuery] = useState("");
    const [rows, setRows] = useState<Row[]>([]);
    const [selected, setSelected] = useState(0);
    const [selectedMode, setMode] = useState<AgentPermissionMode | null>(null);
    const mode = selectedMode ?? (defaultMode === YOLO ? YOLO : NORMAL);

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    useEffect(() => {
        let cancelled = false;
        if (!origin.current.cwd || agents.length === 0) {
            setRows([]);
            return () => {
                cancelled = true;
            };
        }

        const availableAgents = found.filter((agent) => agent.available !== false);
        const sessionArgs = availableAgents.map((agent) => [agent.type, origin.current.cwd, agent.configPath ?? undefined] as const);

        /* The open agents already keep these listings warm, so the picker opens
           on what they last read and swaps in the fresh scan when it lands. */
        const known = sessionArgs.flatMap(([type, cwd, configPath]) =>
            (peekResource(agentSessionsR, type, cwd, configPath) ?? []).map((candidate): Row => ({ ...candidate, type })),
        );
        if (known.length > 0) setRows(sortRows(known));

        void Promise.all(
            sessionArgs.map(([type, cwd, configPath]) =>
                fetchResource(agentSessionsR, type, cwd, configPath)
                    .then((sessions) => sessions.map((candidate): Row => ({ ...candidate, type })))
                    .catch(() => [] as Row[]),
            ),
        ).then((lists) => {
            if (!cancelled) setRows(sortRows(lists.flat()));
        });

        return () => {
            cancelled = true;
        };
    }, [found, agents.length]);

    useEffect(() => {
        if (session && session.id !== origin.current.sessionId) cmd.forceCloseAgentPalette();
    }, [session]);

    /*
     * Capture Escape before the panes behind the scrim can act on it.
     *
     * This used to ride the picker's own `onKeyDown`, which only fires while
     * focus is inside the picker. Anything that took focus first — a terminal
     * pane being the usual one — left the palette unclosable: the terminal saw the
     * key, wrote an escape byte to the PTY, and the dialog never heard it. Same
     * window-capture pattern DialogHost uses, for the same reason.
     */
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            event.stopPropagation();
            cmd.closeAgentPalette();
        };
        window.addEventListener("keydown", onKey, true);
        return () => window.removeEventListener("keydown", onKey, true);
    }, []);

    const items = useMemo(() => {
        const signIns = agents
            .filter((agent) => agent.status?.state === "signedOut" && loginArgs(agent.type))
            .map(({ type }): NewAgentItem => ({ kind: "new", type, signIn: true }));
        const fresh = [...agents.map(({ type }): NewAgentItem => ({ kind: "new", type })), ...signIns];
        const { sessionId, cwd } = origin.current;
        const open = inCoreOrder(
            agentIdsOf({ windowsBySession, windows }, sessionId)
                .map((id) => agentsById[id])
                .filter((agent): agent is Agent => !!agent),
            inbox.agents,
        );
        const resumable = railOrder(
            rows,
            open,
            pinnedChats.filter((pin) => pin.project === cwd),
        );
        const rankedFresh = rankBy(query, fresh, (item) =>
            item.signIn
                ? `sign in to ${agentLabel(item.type)} ${item.type}`
                : installable(
                        item,
                        agents.find((agent) => agent.type === item.type),
                    )
                  ? `install ${agentLabel(item.type)} ${item.type}`
                  : `+ new ${agentLabel(item.type)} ${item.type}`,
        );
        const rankedResumable = rankBy(
            query,
            resumable,
            (item) => `${item.row.title} ${agentLabel(item.row.type)} ${item.row.type}${item.agent?.name ? ` ${agentHandle(item.agent.name)}` : ""}`,
        );
        return [...rankedFresh, ...rankedResumable];
    }, [agents, query, rows, windowsBySession, windows, agentsById, pinnedChats, inbox.agents]);

    const selectable = useMemo(
        () =>
            items
                .map((item, index) => {
                    const type = typeForItem(item);
                    const provider = agents.find((agent) => agent.type === type);
                    return { index, available: provider?.available !== false || installable(item, provider) };
                })
                .filter(({ available }) => available)
                .map(({ index }) => index),
        [agents, items],
    );
    const firstResumeIndex = items.findIndex((item) => item.kind === "resume");

    useEffect(() => {
        setSelected((current) => (selectable.includes(current) ? current : (selectable[0] ?? 0)));
    }, [selectable]);

    function chooseMode(nextMode: AgentPermissionMode) {
        setMode(nextMode);
        window.requestAnimationFrame(() => inputRef.current?.focus());
    }

    function launchMode(type: AgentType): AgentPermissionMode {
        return mode === YOLO && !canBypassPermissions(type) ? NORMAL : mode;
    }

    function moveSelection(delta: number) {
        if (selectable.length === 0) return;
        const current = selectable.indexOf(selected);
        const base = current < 0 ? (delta > 0 ? -1 : 0) : current;
        setSelected(selectable[(base + delta + selectable.length) % selectable.length]);
    }

    function activate(item: AgentItem | undefined) {
        if (!item || !origin.current.sessionId || !origin.current.cwd) return;
        const type = typeForItem(item);
        const provider = agents.find((agent) => agent.type === type);
        if (installable(item, provider)) {
            cmd.closeAgentPalette();
            installAgent(type);
            return;
        }
        if (item.kind === "new" && item.signIn && provider) {
            cmd.closeAgentPalette();
            signInAgent(provider);
            return;
        }
        if (item.kind === "resume" && item.agent) {
            cmd.closeAgentPalette();
            cmd.revealAgent(item.agent.id);
            return;
        }
        if (!provider || provider.available === false) return;
        const selectedProfile = selectedProviderProfile(type, profiles, profileSelections);
        const resume = item.kind === "resume" ? item.row : undefined;
        cmd.addAgent(type, resume?.id, resume?.title, {
            permissionMode: launchMode(type),
            profileId: selectedProfile?.id,
            detectedExecutablePath: provider.command,
            cwd: origin.current.cwd,
            sessionId: origin.current.sessionId,
        });
    }

    // Escape is handled by the window-capture listener above, so that it works
    // regardless of what holds focus.
    function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
        if (event.target !== inputRef.current) return;
        if (event.key === "ArrowDown" || (event.key === "Tab" && !event.shiftKey)) {
            event.preventDefault();
            moveSelection(1);
        } else if (event.key === "ArrowUp" || (event.key === "Tab" && event.shiftKey)) {
            event.preventDefault();
            moveSelection(-1);
        } else if (event.key === "Enter") {
            event.preventDefault();
            activate(items[selected]);
        }
    }

    const emptyMessage =
        catalog.status === "loading"
            ? "detecting agent CLIs..."
            : catalog.status === "error"
              ? catalog.error || "agent detection failed"
              : "no agent matches";

    return (
        <div ref={leavingOverlay} className="picker-backdrop" onMouseDown={cmd.closeAgentPalette}>
            <div
                ref={modalRef}
                tabIndex={-1}
                className="picker agent-palette"
                role="dialog"
                aria-modal="true"
                aria-label="Open agent CLI"
                onKeyDown={onKeyDown}
                onMouseDown={(event) => event.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        aria-label="Search agent sessions"
                        placeholder={agents.length ? "search agents" : emptyMessage}
                        value={query}
                        onChange={(event) => {
                            setQuery(event.target.value);
                            setSelected(0);
                        }}
                        spellCheck={false}
                    />
                    <div className="yolo-switch" role="radiogroup" aria-label="Agent safety boundary">
                        {MODE_CHOICES.map((choice) => (
                            <button
                                key={choice.mode}
                                type="button"
                                role="radio"
                                aria-checked={mode === choice.mode}
                                className={`yolo-switch-option${choice.mode === YOLO ? " armed" : ""}${mode === choice.mode ? " active" : ""}`}
                                title={choice.title}
                                onClick={() => chooseMode(choice.mode)}>
                                <span className="yolo-switch-glyph" aria-hidden="true">
                                    {choice.mode === YOLO ? <IconShieldBolt size={12} /> : <IconShield size={12} />}
                                </span>
                                <span className="yolo-switch-label">{choice.label}</span>
                            </button>
                        ))}
                    </div>
                </div>

                <div className="picker-list">
                    {items.length === 0 && (
                        <div className="picker-empty" role="status">
                            {emptyMessage}
                            {catalog.status === "error" && (
                                <button type="button" onClick={() => void catalog.refresh().catch(swallow("refresh the agent catalog"))}>
                                    try again
                                </button>
                            )}
                        </div>
                    )}
                    {items.map((item, index) => {
                        const type = typeForItem(item);
                        const provider = agents.find((agent) => agent.type === type);
                        const install = installable(item, provider);
                        const available = provider?.available !== false || install;
                        const display = item.kind === "resume" && item.agent ? agentStatus[item.agent.id] : undefined;
                        const signIn = item.kind === "new" && !!item.signIn;
                        const key = item.kind === "new" ? `${signIn ? "sign-in" : "new"}-${type}` : `${type}-${item.row.id}`;
                        const name = signIn
                            ? `Sign in to ${agentLabel(type)}`
                            : install
                              ? `Install ${agentLabel(type)}`
                              : item.kind === "new"
                                ? `+ new ${agentLabel(type)}`
                                : item.row.title;
                        return (
                            <Fragment key={key}>
                                {index === firstResumeIndex && firstResumeIndex > 0 && <div className="agent-palette-divider" />}
                                <button
                                    type="button"
                                    className={`picker-item${index === selected ? " sel" : ""}`}
                                    disabled={!available}
                                    aria-label={install || signIn ? name : `${name} in ${launchMode(type) === YOLO ? "YOLO" : "Normal"} mode`}
                                    onMouseEnter={() => {
                                        if (mouseActive.current && available) setSelected(index);
                                    }}
                                    onClick={() => activate(item)}>
                                    {item.kind === "resume" && item.agent ? (
                                        <AgentFace
                                            agent={item.agent}
                                            size={14}
                                            logoSize={14}
                                            logoClass={`picker-icon agent-glyph ${type}`}
                                            buddyClass="picker-icon agent-glyph buddy-mark"
                                        />
                                    ) : (
                                        <span className={`picker-icon agent-glyph ${type}`}>
                                            <AgentIcon type={type} size={14} />
                                        </span>
                                    )}
                                    <span className="picker-name">{name}</span>
                                    {item.kind === "resume" && item.agent?.name && <span className="picker-sub">{agentHandle(item.agent.name)}</span>}
                                    {showsAgentState(display) ? (
                                        <span className="picker-state">
                                            <AgentStateIndicator display={display} />
                                        </span>
                                    ) : (
                                        <span className="picker-sub">
                                            {signIn
                                                ? "Signed out"
                                                : install
                                                  ? agentAbout(type)
                                                  : !available
                                                    ? provider?.error || "Agent executable is unavailable"
                                                    : launchMode(type) !== mode
                                                      ? "opens safe"
                                                      : item.kind === "new"
                                                        ? provider?.status?.state === "signedOut"
                                                            ? "signed out"
                                                            : ""
                                                        : item.row.mtime
                                                          ? formatAge(item.row.mtime * 1000)
                                                          : ""}
                                        </span>
                                    )}
                                </button>
                            </Fragment>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}

/** A new-chat row for an agent that is not installed, which installs it instead. */
function installable(item: AgentItem, provider: AgentInfo | undefined): boolean {
    return item.kind === "new" && provider?.status?.state === "missing" && !!installCommand(item.type);
}
