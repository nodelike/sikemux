import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AgentInfo, AgentUsage, AgentUsageWindow } from "../api/agents";
import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "../agentProfiles";
import * as cmd from "../state/commands";
import { type ResourceHandle, useResource, useResourceEnabled } from "../state/resources";
import { agentCatalogR, agentSessionsR, agentUsageR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { type Agent, type AgentType } from "../state/types";
import { AgentIcon, IconClose, IconPlus, IconRefresh, IconSearch } from "./Icons";
import { AgentStateIndicator } from "./AgentStateIndicator";
import { Tooltip } from "./Tooltip";
import { Panel, PanelHeader } from "./Panel";

const RECENTS_PAGE = 12;
const USAGE_REFRESH_MS = 5 * 60_000;
type UsageAgentType = "claude" | "codex";

function isUsageAgent(type: AgentType | null): type is UsageAgentType {
    return type === "claude" || type === "codex";
}

function ago(unixSecs: number): string {
    if (!unixSecs) return "";
    const d = Math.max(0, Date.now() / 1000 - unixSecs);
    if (d < 90) return "now";
    if (d < 3600) return `${Math.round(d / 60)}m`;
    if (d < 86400) return `${Math.round(d / 3600)}h`;
    return `${Math.round(d / 86400)}d`;
}

const persistedSessionIdOf = (a: Agent) => a.resumeId ?? a.id;
const sessionKey = (type: AgentType, id: string) => `${type}:${id}`;

export function AgentRailBody() {
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const activityById = useStore((s) => s.agentActivity);
    const agentsBySession = useStore((s) => s.agentsBySession);
    const agentsById = useStore((s) => s.agents);
    const profiles = useStore((s) => s.providerProfiles);
    const profileSelections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    const catalogAgents = useMemo(() => catalog.data ?? [], [catalog.data]);
    const availableAgents = useMemo(() => catalogAgents.filter((agent) => agent.available !== false), [catalogAgents]);
    const availableTypes = useMemo(() => new Set(availableAgents.map((a) => a.type)), [availableAgents]);
    const claudeDetected = availableTypes.has("claude");
    const codexDetected = availableTypes.has("codex");
    const claudeProvider = availableAgents.find((agent) => agent.type === "claude");
    const codexProvider = availableAgents.find((agent) => agent.type === "codex");
    const claudeUsage = useResourceEnabled(claudeDetected, agentUsageR, "claude", claudeProvider?.command, claudeProvider?.configPath ?? undefined);
    const codexUsage = useResourceEnabled(codexDetected, agentUsageR, "codex", codexProvider?.command, codexProvider?.configPath ?? undefined);
    const usageRefreshRef = useRef({ claude: claudeUsage.refresh, codex: codexUsage.refresh });
    usageRefreshRef.current = { claude: claudeUsage.refresh, codex: codexUsage.refresh };

    const [type, setType] = useState<AgentType | null>(null);
    const [visibleRecents, setVisibleRecents] = useState(RECENTS_PAGE);
    // Recent chats live here and nowhere else, so the search for them does too.
    const [query, setQuery] = useState("");
    const [searchOpen, setSearchOpen] = useState(false);
    const searchRef = useRef<HTMLInputElement>(null);
    const scrollRef = useRef<HTMLDivElement>(null);
    const selectedType = useMemo(() => {
        if (type && availableAgents.some((a) => a.type === type)) return type;
        return availableAgents[0]?.type ?? null;
    }, [availableAgents, type]);

    useEffect(() => {
        if (selectedType !== type) setType(selectedType);
    }, [selectedType, type]);

    useEffect(() => {
        if (searchOpen) searchRef.current?.focus();
    }, [searchOpen]);

    useEffect(() => {
        if (!claudeDetected && !codexDetected) return;
        const timer = window.setInterval(() => {
            if (claudeDetected) void usageRefreshRef.current.claude();
            if (codexDetected) void usageRefreshRef.current.codex();
        }, USAGE_REFRESH_MS);
        return () => window.clearInterval(timer);
    }, [claudeDetected, codexDetected]);

    const isProject = session?.kind === "project";
    const cwd = session?.cwd ?? "";

    const selectedProvider = availableAgents.find((agent) => agent.type === selectedType);
    const recents = useResourceEnabled(
        isProject && !!cwd && selectedType != null,
        agentSessionsR,
        selectedType ?? "claude",
        isProject ? cwd : "",
        selectedProvider?.configPath ?? undefined,
    );
    const disk = isProject ? (recents.data ?? []) : [];
    const selectedUsage = selectedType === "claude" ? claudeUsage : selectedType === "codex" ? codexUsage : null;
    const usagePeaks = {
        claude: usagePeak(claudeUsage.data),
        codex: usagePeak(codexUsage.data),
    };

    // Reset the reveal window when the recents list switches out from under us.
    useEffect(() => {
        setVisibleRecents(RECENTS_PAGE);
    }, [selectedType, cwd, query]);

    // onRailScroll only reveals more once the list overflows. If the first page
    // doesn't reach the bottom there's no scrollbar, so the rest would never load
    // and the rail sits half-empty. Reveal more until it fills — and re-check when
    // the rail is resized taller.
    useLayoutEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        const fill = () => {
            if (el.scrollHeight <= el.clientHeight && visibleRecents < disk.length) {
                setVisibleRecents((v) => Math.min(v + RECENTS_PAGE, disk.length));
            }
        };
        fill();
        const ro = new ResizeObserver(fill);
        ro.observe(el);
        return () => ro.disconnect();
    }, [visibleRecents, disk.length, cwd, selectedType]);

    if (!session) return null;

    const opens = ((agentsBySession[session.id] ?? []).map((id) => agentsById[id]).filter(Boolean) as Agent[]).filter((a) =>
        availableTypes.has(a.type),
    );

    const activeOpenKeys = new Set(opens.map((a) => sessionKey(a.type, persistedSessionIdOf(a))));
    const needle = query.trim().toLowerCase();
    const recentAll = disk.filter((d) => {
        if (!selectedType) return false;
        if (needle && !d.title.toLowerCase().includes(needle)) return false;
        const k = sessionKey(selectedType, d.id);
        return !activeOpenKeys.has(k);
    });
    const recentDisplay = recentAll.slice(0, visibleRecents);
    const hasMoreRecents = recentDisplay.length < recentAll.length;

    const onRailScroll = () => {
        if (!hasMoreRecents) return;
        const el = scrollRef.current;
        if (!el) return;
        if (el.scrollHeight - el.scrollTop - el.clientHeight < 120) {
            setVisibleRecents((v) => Math.min(v + RECENTS_PAGE, recentAll.length));
        }
    };

    const toggleSearch = () => {
        setQuery("");
        setSearchOpen((open) => !open);
    };

    if (!isProject) {
        return (
            <>
                <AgentHeader
                    agents={availableAgents}
                    type={selectedType}
                    setType={setType}
                    searchOpen={false}
                    onToggleSearch={toggleSearch}
                    usagePeaks={usagePeaks}
                    canOpenPalette={catalogAgents.length > 0}
                />
                <div className="agent-empty">agents are project-scoped</div>
                {isUsageAgent(selectedType) && selectedUsage && (
                    <AgentUsagePanel
                        provider={selectedType}
                        usage={selectedUsage}
                        label={availableAgents.find((a) => a.type === selectedType)?.label}
                    />
                )}
            </>
        );
    }

    const noContent = opens.length === 0 && recentDisplay.length === 0;

    return (
        <>
            <AgentHeader
                agents={availableAgents}
                type={selectedType}
                setType={setType}
                searchOpen={searchOpen}
                onToggleSearch={toggleSearch}
                usagePeaks={usagePeaks}
                canOpenPalette={catalogAgents.length > 0}
            />
            {searchOpen && (
                <div className="rail-search">
                    <IconSearch size={12} />
                    <input
                        ref={searchRef}
                        value={query}
                        onChange={(e) => setQuery(e.target.value)}
                        onKeyDown={(e) => {
                            if (e.key === "Escape") toggleSearch();
                            e.stopPropagation();
                        }}
                        placeholder="filter recent chats…"
                        aria-label="Filter recent chats"
                        spellCheck={false}
                    />
                </div>
            )}
            <div className="rail-scroll" ref={scrollRef} onScroll={onRailScroll}>
                {noContent && (
                    <div className="agent-empty">
                        {catalog.status === "loading"
                            ? "detecting agent CLIs..."
                            : availableAgents.length === 0
                              ? "no agent CLIs detected on PATH"
                              : needle
                                ? "no recent chats match this filter"
                                : "no agents yet — start one above"}
                    </div>
                )}

                {opens.length > 0 && (
                    <Panel variant="group" className="agent-group">
                        <PanelHeader label="Open" rule />
                        {opens.map((a) => {
                            const active = session.view === "agent" && a.id === session.activeAgentId;
                            return (
                                <div key={a.id} className="agent-row-wrap">
                                    <button className={`agent-row closable${active ? " active" : ""}`} onClick={() => cmd.selectAgent(a.id)}>
                                        <span className={`agent-glyph ${a.type}`}>
                                            <span className="agent-glyph-icon">
                                                <AgentIcon type={a.type} size={20} />
                                            </span>
                                        </span>
                                        <span className="agent-title">{a.title}</span>
                                        {activityById[a.id] && <AgentStateMark state={activityById[a.id].state} />}
                                        {a.launchState === "dormant" && <span className="agent-dormant-label">paused</span>}
                                    </button>
                                    <Tooltip label={`Close ${a.title}`}>
                                        <button
                                            type="button"
                                            className="agent-glyph-x"
                                            aria-label={`Close ${a.title}`}
                                            onClick={() => cmd.closeAgent(a.id)}>
                                            <IconClose size={11} />
                                        </button>
                                    </Tooltip>
                                </div>
                            );
                        })}
                    </Panel>
                )}

                {selectedType && recentDisplay.length > 0 && (
                    <Panel variant="group" className="agent-group">
                        <PanelHeader label="Recent" rule />
                        {recentDisplay.map((s) => (
                            <button
                                key={s.id}
                                className="agent-row recent"
                                onClick={() =>
                                    cmd.addAgent(selectedType, s.id, s.title, {
                                        profileId: selectedProviderProfile(selectedType, profiles, profileSelections)?.id,
                                        detectedExecutablePath: selectedProvider?.command,
                                    })
                                }>
                                <span className={`agent-glyph ${selectedType}`}>
                                    <AgentIcon type={selectedType} size={20} />
                                </span>
                                <span className="agent-title">{s.title}</span>
                                <span className="agent-ago">{ago(s.mtime)}</span>
                            </button>
                        ))}
                    </Panel>
                )}
            </div>
            {/* The rail's footer: plan limits sit under the agents they apply
                to, out of the way of the list you came here to use. */}
            {isUsageAgent(selectedType) && selectedUsage && (
                <AgentUsagePanel provider={selectedType} usage={selectedUsage} label={availableAgents.find((a) => a.type === selectedType)?.label} />
            )}
        </>
    );
}

function AgentStateMark({ state }: { state: import("../state/types").AgentPresentationState }) {
    return <AgentStateIndicator state={state} />;
}

function usagePeak(usage: AgentUsage | undefined): number | undefined {
    if (!usage?.windows.length) return undefined;
    return Math.max(...usage.windows.map((window) => Math.max(0, Math.min(100, window.usedPercent))));
}

function usageTone(percent: number): "steady" | "warm" | "hot" {
    if (percent >= 90) return "hot";
    if (percent >= 70) return "warm";
    return "steady";
}

function resetAtMs(value: AgentUsageWindow["resetsAt"]): number | null {
    if (typeof value === "number") return Number.isFinite(value) ? value * 1000 : null;
    if (typeof value !== "string" || !value) return null;
    if (/^\d+$/.test(value)) return Number(value) * 1000;
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? null : parsed;
}

function resetCountdown(value: AgentUsageWindow["resetsAt"], now: number): string {
    const reset = resetAtMs(value);
    if (reset == null) return "reset unknown";
    const minutes = Math.max(0, Math.ceil((reset - now) / 60_000));
    if (minutes === 0) return "resetting now";
    if (minutes < 60) return `reset ${minutes}m`;
    const hours = Math.floor(minutes / 60);
    const remainingMinutes = minutes % 60;
    if (hours < 24) return `reset ${hours}h${remainingMinutes ? ` ${remainingMinutes}m` : ""}`;
    const days = Math.floor(hours / 24);
    const remainingHours = hours % 24;
    if (days < 7) return `reset ${days}d${remainingHours ? ` ${remainingHours}h` : ""}`;
    return `reset ${new Date(reset).toLocaleDateString(undefined, { month: "short", day: "numeric" })}`;
}

function resetTitle(value: AgentUsageWindow["resetsAt"]): string {
    const reset = resetAtMs(value);
    return reset == null ? "Reset time unavailable" : `Resets ${new Date(reset).toLocaleString()}`;
}

function planLabel(plan: string): string {
    return plan
        .split(/[_-]/g)
        .filter(Boolean)
        .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
        .join(" ");
}

function AgentUsagePanel({ provider, usage, label }: { provider: UsageAgentType; usage: ResourceHandle<AgentUsage>; label?: string }) {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        const timer = window.setInterval(() => setNow(Date.now()), 60_000);
        return () => window.clearInterval(timer);
    }, []);

    const providerLabel = label ?? (provider === "claude" ? "Claude" : "Codex");
    const windows = usage.data?.windows ?? [];
    const emptyCopy =
        usage.status === "loading"
            ? "reading plan limits…"
            : usage.status === "error"
              ? "Could not read plan limits. Refresh to try again."
              : (usage.data?.unavailableReason ?? "This account did not report plan limits.");

    return (
        <section className={`agent-usage ${provider}`} aria-label={`${providerLabel} plan limits`}>
            <div className="panel-head agent-usage-head">
                <span className="panel-label">Limits</span>
                <span className="panel-rule" />
                {usage.data?.plan && <span className="agent-usage-plan">{planLabel(usage.data.plan)}</span>}
                <Tooltip label={`Refresh ${providerLabel} plan limits`}>
                    <button
                        type="button"
                        className="rail-group-add"
                        aria-label={`Refresh ${providerLabel} plan limits`}
                        disabled={usage.status === "loading"}
                        onClick={() => void usage.refresh()}>
                        <IconRefresh size={11} />
                    </button>
                </Tooltip>
            </div>

            {windows.length > 0 ? (
                windows.map((window, index) => {
                    const percent = Math.max(0, Math.min(100, window.usedPercent));
                    const rounded = Math.round(percent);
                    return (
                        <Tooltip
                            key={`${window.label}:${String(window.resetsAt)}:${index}`}
                            side="left"
                            label={`${window.label}: ${rounded}% used. ${resetTitle(window.resetsAt)}`}>
                            <div className="agent-usage-row" data-tone={usageTone(percent)}>
                                <div className="agent-usage-line">
                                    <span className="agent-usage-name">{window.label}</span>
                                    <span className="agent-usage-reset">{resetCountdown(window.resetsAt, now)}</span>
                                </div>
                                <div className="agent-usage-gauge">
                                    <span className="agent-usage-pct">
                                        {rounded}
                                        <i>%</i>
                                    </span>
                                    <span
                                        className="agent-usage-track"
                                        role="meter"
                                        aria-label={`${window.label} usage`}
                                        aria-valuemin={0}
                                        aria-valuemax={100}
                                        aria-valuenow={rounded}>
                                        <span className="agent-usage-fill" style={{ width: `${percent}%` }} />
                                    </span>
                                </div>
                            </div>
                        </Tooltip>
                    );
                })
            ) : (
                <div className="agent-usage-empty" data-loading={usage.status === "loading" ? "true" : "false"}>
                    {emptyCopy}
                </div>
            )}
        </section>
    );
}

function AgentHeader({
    agents,
    type,
    setType,
    searchOpen,
    onToggleSearch,
    usagePeaks,
    canOpenPalette,
}: {
    agents: AgentInfo[];
    type: AgentType | null;
    setType: (t: AgentType) => void;
    searchOpen: boolean;
    onToggleSearch: () => void;
    usagePeaks: Partial<Record<UsageAgentType, number | undefined>>;
    canOpenPalette: boolean;
}) {
    const label = agents.find((a) => a.type === type)?.label ?? type;
    return (
        <div className="agent-header">
            <div className="agent-header-top">
                <span className="agent-header-label">Agents</span>
                <div className="agent-header-actions">
                    <Tooltip label="Filter recent chats">
                        <button
                            className={`agent-header-btn${searchOpen ? " active" : ""}`}
                            aria-pressed={searchOpen}
                            aria-label="Filter recent chats"
                            onClick={onToggleSearch}>
                            <IconSearch size={15} />
                        </button>
                    </Tooltip>
                    <Tooltip label={type ? `new ${label} agent — ⌥N` : canOpenPalette ? "Review agent setup" : "No agent CLI detected"}>
                        <button
                            className="agent-header-btn"
                            disabled={!type && !canOpenPalette}
                            aria-label={type ? `New ${label} agent` : canOpenPalette ? "Review agent setup" : "No agent CLI detected"}
                            onClick={() => {
                                if (type || canOpenPalette) cmd.openAgentPalette();
                            }}>
                            <IconPlus size={15} />
                        </button>
                    </Tooltip>
                </div>
            </div>
            <div className="agent-header-types" role="tablist" aria-label="Agent provider">
                {agents.map((a) => (
                    <Tooltip
                        key={a.type}
                        label={
                            isUsageAgent(a.type) && usagePeaks[a.type] != null
                                ? `${a.label} — ${Math.round(usagePeaks[a.type]!)}% of the busiest limit used`
                                : a.label
                        }>
                        <button
                            role="tab"
                            aria-selected={type === a.type}
                            className={`agent-header-btn ${a.type}${type === a.type ? " active" : ""}`}
                            aria-label={a.label}
                            onClick={() => setType(a.type)}>
                            <AgentIcon type={a.type} size={18} />
                        </button>
                    </Tooltip>
                ))}
            </div>
        </div>
    );
}
