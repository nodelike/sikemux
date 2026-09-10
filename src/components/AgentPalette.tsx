import { useModalFocus } from "../hooks/useModalFocus";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { agentApi, type AgentInfo, type AgentSession } from "../api/agents";
import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "../agentProfiles";
import { useMouseActive } from "../hooks/useMouseActive";
import { rankBy } from "../lib/fuzzy";
import * as cmd from "../state/commands";
import { useResource } from "../state/resources";
import { agentCatalogR } from "../state/resources.defs";
import { useStore } from "../state/store";
import type { AgentPermissionMode, AgentType } from "../state/types";
import { AgentIcon, IconSearch, IconShield, IconShieldBolt } from "./Icons";

type Row = AgentSession & { type: AgentType };
type NewAgentItem = { kind: "new"; type: AgentType };
type ResumeAgentItem = { kind: "resume"; row: Row };
type AgentItem = NewAgentItem | ResumeAgentItem;

const NORMAL: AgentPermissionMode = "workspace-write";
const YOLO: AgentPermissionMode = "bypass";

const MODE_CHOICES: { mode: AgentPermissionMode; label: string; title: string }[] = [
    { mode: NORMAL, label: "safe", title: "Safe mode — the agent launches with normal approvals." },
    { mode: YOLO, label: "yolo", title: "YOLO mode — the agent launches without approvals." },
];

function labelForType(type: AgentType, agents: readonly AgentInfo[]): string {
    return agents.find((agent) => agent.type === type)?.label ?? type;
}

function typeForItem(item: AgentItem): AgentType {
    return item.kind === "new" ? item.type : item.row.type;
}

function ago(unixSecs: number): string {
    if (!unixSecs) return "";
    const delta = Math.max(0, Date.now() / 1000 - unixSecs);
    if (delta < 90) return "now";
    if (delta < 3600) return `${Math.round(delta / 60)}m`;
    if (delta < 86400) return `${Math.round(delta / 3600)}h`;
    return `${Math.round(delta / 86400)}d`;
}

export function AgentPalette() {
    const modalRef = useRef<HTMLDivElement>(null);
    useModalFocus(modalRef);
    const session = useStore((state) => state.sessions[state.activeSessionId]);
    const profiles = useStore((state) => state.providerProfiles);
    const profileSelections = useStore((state) => state.selectedProviderProfileIds);
    const defaultMode = useStore((state) => state.defaultAgentPermissionMode);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    const agents = useMemo(() => catalog.data ?? [], [catalog.data]);
    const origin = useRef({ sessionId: session?.id ?? "", cwd: session?.cwd ?? "" });
    const inputRef = useRef<HTMLInputElement>(null);
    const mouseActive = useMouseActive();
    const [query, setQuery] = useState("");
    const [rows, setRows] = useState<Row[]>([]);
    const [selected, setSelected] = useState(0);
    const [mode, setMode] = useState<AgentPermissionMode>(defaultMode === YOLO ? YOLO : NORMAL);

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

        // Hermes history is global rather than project-scoped, so showing it
        // here leaks unrelated projects into a picker opened for one checkout.
        const projectScopedAgents = agents.filter((agent) => agent.available !== false && agent.type !== "hermes");
        void Promise.all(
            projectScopedAgents.map((agent) =>
                agentApi
                    .sessions(agent.type, origin.current.cwd, agent.configPath ?? undefined)
                    .then((sessions) => sessions.map((candidate): Row => ({ ...candidate, type: agent.type })))
                    .catch(() => [] as Row[]),
            ),
        ).then((lists) => {
            if (!cancelled) setRows(lists.flat().sort((left, right) => right.mtime - left.mtime));
        });

        return () => {
            cancelled = true;
        };
    }, [agents]);

    useEffect(() => {
        if (session && session.id !== origin.current.sessionId) cmd.forceCloseAgentPalette();
    }, [session]);

    /*
     * Capture Escape before the panes behind the scrim can act on it.
     *
     * This used to ride the picker's own `onKeyDown`, which only fires while
     * focus is inside the picker. Anything that took focus first — a terminal
     * pane being the usual one — left the palette unclosable: xterm saw the
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
        const fresh = agents.map(({ type }): NewAgentItem => ({ kind: "new", type }));
        const resumable = rows.map((row): ResumeAgentItem => ({ kind: "resume", row }));
        const rankedFresh = rankBy(query, fresh, (item) => `+ new ${labelForType(item.type, agents)} ${item.type}`);
        const rankedResumable = rankBy(query, resumable, (item) => `${item.row.title} ${labelForType(item.row.type, agents)} ${item.row.type}`);
        return [...rankedFresh, ...rankedResumable];
    }, [agents, query, rows]);

    const selectable = useMemo(
        () =>
            items
                .map((item, index) => {
                    const type = typeForItem(item);
                    const available = agents.find((agent) => agent.type === type)?.available !== false;
                    return { index, supported: available && (mode === NORMAL || cmd.agentSupportsSkipPermissions(type)) };
                })
                .filter(({ supported }) => supported)
                .map(({ index }) => index),
        [agents, items, mode],
    );
    const firstResumeIndex = items.findIndex((item) => item.kind === "resume");

    useEffect(() => {
        setSelected((current) => (selectable.includes(current) ? current : (selectable[0] ?? 0)));
    }, [selectable]);

    function chooseMode(nextMode: AgentPermissionMode) {
        setMode(nextMode);
        window.requestAnimationFrame(() => inputRef.current?.focus());
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
        if (!provider || provider.available === false) return;
        if (mode === YOLO && !cmd.agentSupportsSkipPermissions(type)) return;
        const selectedProfile = selectedProviderProfile(type, profiles, profileSelections);
        const resume = item.kind === "resume" ? item.row : undefined;
        cmd.addAgent(type, resume?.id, resume?.title, {
            permissionMode: mode,
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
        <div className="picker-backdrop" onMouseDown={cmd.closeAgentPalette}>
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
                        placeholder={agents.length ? `search agent sessions — ${agents.map((agent) => agent.label).join(" · ")}...` : emptyMessage}
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
                                <button type="button" onClick={() => void catalog.refresh().catch(() => {})}>
                                    try again
                                </button>
                            )}
                        </div>
                    )}
                    {items.map((item, index) => {
                        const type = typeForItem(item);
                        const provider = agents.find((agent) => agent.type === type);
                        const available = provider?.available !== false;
                        const supported = available && (mode === NORMAL || cmd.agentSupportsSkipPermissions(type));
                        const key = item.kind === "new" ? `new-${type}` : `${type}-${item.row.id}`;
                        const name = item.kind === "new" ? `+ new ${labelForType(type, agents)}` : item.row.title;
                        return (
                            <Fragment key={key}>
                                {index === firstResumeIndex && firstResumeIndex > 0 && <div className="agent-palette-divider" />}
                                <button
                                    type="button"
                                    className={`picker-item${index === selected ? " sel" : ""}`}
                                    disabled={!supported}
                                    aria-label={`${name} in ${mode === YOLO ? "YOLO" : "Normal"} mode`}
                                    onMouseEnter={() => {
                                        if (mouseActive.current && supported) setSelected(index);
                                    }}
                                    onClick={() => activate(item)}>
                                    <span className={`picker-icon agent-glyph ${type}`}>
                                        <AgentIcon type={type} size={14} />
                                    </span>
                                    <span className="picker-name">{name}</span>
                                    <span className="picker-sub">
                                        {!available
                                            ? provider?.error || "Agent executable is unavailable"
                                            : !supported
                                              ? "Normal mode only"
                                              : item.kind === "new"
                                                ? "start agent"
                                                : `${labelForType(type, agents)} · ${ago(item.row.mtime)}`}
                                    </span>
                                </button>
                            </Fragment>
                        );
                    })}
                </div>
            </div>
        </div>
    );
}
