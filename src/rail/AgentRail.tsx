import { navigateTabs } from "../lib/tabNavigation";
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AgentInfo, AgentUsage, AgentUsageWindow } from "../api/agents";
import { usePageVisible } from "../hooks/usePageVisible";
import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "../agents/agentProfiles";
import * as cmd from "../state/commands";
import { useShortcutLabel, withShortcut } from "../commands/useShortcutLabel";
import { type ResourceHandle, useResource, useResourceEnabled } from "../state/resources";
import { agentCatalogR, agentUsageR } from "../state/resources.defs";
import { useStore } from "../state/store";
import { activeAgentId, agentIdsOf, agentsAwaitingInput } from "../state/selectors";
import { type Agent, type AgentType, type ProviderProfile, type ProviderProfileSelection } from "../state/types";
import { AgentIcon, IconClose, IconInbox, IconPlus, IconRefresh, IconSearch } from "../ui/Icons";
import { AgentStateIndicator } from "../agents/AgentStateIndicator";
import { AgentTitleInput } from "../agents/AgentTitleInput";
import { AgentContextMenu } from "../workspace/AgentContextMenu";
import { sortByAttention } from "../state/agentStatus";
import { Tooltip } from "../ui/Tooltip";
import { Dropdown } from "../ui/Dropdown";
import { Panel, PanelHeader } from "../ui/Panel";
import { animate, type Box, contentBox, EASE_LEAVE, glideSelection, leavingRef } from "../lib/motion";
import { CountUp } from "../ui/RollingText";
import { leavingRail } from "./railMotion";
import { RailToggle } from "./RailToggle";
import { AllProjectsAgents } from "./AllProjectsAgents";
import { RecentChatList } from "./RecentChatList";
import { ScopeTrack } from "./ScopeTrack";
import { useRecentChats } from "./useRecentChats";
import { LEAVES_SETTINGS } from "../settings/leaveSettings";

const USAGE_REFRESH_MS = 5 * 60_000;
type UsageAgentType = "claude" | "codex";

function isUsageAgent(type: AgentType | null): type is UsageAgentType {
    return type === "claude" || type === "codex";
}

const persistedSessionIdOf = (a: Agent) => a.resumeId ?? a.id;

/**
 * An open agent's row, closing: content goes in 50ms and the slot closes
 * front-loaded, so the row answers the click at once. Rows swapped out by a
 * project switch just go, and a closed selected row leaves its box behind for
 * the selection to glide from.
 */
function rowLeaving(currentSession: { current: string }, closedBoxes: Map<string, Box>) {
    return leavingRef<HTMLDivElement>(
        (wrap) => {
            wrap.style.overflow = "hidden";
            for (const part of wrap.children)
                animate(part, [{ opacity: 1 }, { opacity: 0, transform: "translateX(-6px)" }], { duration: 50, easing: "linear", fill: "forwards" });
            return animate(wrap, [{ height: `${wrap.offsetHeight}px` }, { height: "0px", marginTop: "0px", marginBottom: "0px" }], {
                duration: 120,
                easing: EASE_LEAVE,
            });
        },
        {
            onRemove: (wrap) => {
                if (wrap.dataset.session !== currentSession.current) return false;
                const row = wrap.querySelector<HTMLElement>(".agent-row.active");
                const group = wrap.parentElement;
                if (row && group && wrap.dataset.agentId) closedBoxes.set(wrap.dataset.agentId, contentBox(row.getBoundingClientRect(), group));
            },
        },
    );
}

function arriveRow(wrap: HTMLElement): void {
    const height = wrap.offsetHeight;
    animate(
        wrap,
        [
            { height: "0px", opacity: 0, marginTop: "0px", marginBottom: "0px" },
            { height: `${height}px`, opacity: 1 },
        ],
        { duration: 170 },
    );
    animate(wrap.querySelector(".agent-row"), [{ transform: "translateX(-6px)" }, { transform: "none" }], { duration: 190 });
}

export const AgentRail = memo(function AgentRail() {
    const density = useStore((s) => s.railDensity);
    return (
        <aside ref={leavingRail} className="workspace-rail agent-rail" aria-label="Agents" data-density={density} {...LEAVES_SETTINGS}>
            <AgentRailBody />
        </aside>
    );
});

export function AgentRailBody() {
    const pageVisible = usePageVisible();
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const activityById = useStore((s) => s.agentActivity);
    const backgroundById = useStore((s) => s.agentBackgroundWork);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const windowsById = useStore((s) => s.windows);
    const agentsById = useStore((s) => s.agents);
    const profiles = useStore((s) => s.providerProfiles);
    const profileSelections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    // Switching account re-detects the CLIs; the last answer holds the rail steady meanwhile.
    const [lastCatalog, setLastCatalog] = useState<AgentInfo[]>([]);
    if (catalog.data && catalog.data !== lastCatalog) setLastCatalog(catalog.data);
    const catalogAgents = catalog.data ?? lastCatalog;
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
    const allAgents = useStore((s) => s.agentRailAllAgents);
    const scope = useStore((s) => s.agentRailScope);
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const agentActivity = useStore((s) => s.agentActivity);
    // Recent chats live here and nowhere else, so the search for them does too.
    const [query, setQuery] = useState("");
    const [searchOpen, setSearchOpen] = useState(false);
    const [renamingId, setRenamingId] = useState<string | null>(null);
    const [menu, setMenu] = useState<{ agentId: string; x: number; y: number } | null>(null);
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

    /*
     * Each refresh boots the provider's CLI to ask what the plan has left, so
     * it only runs while someone can see the answer.
     */
    useEffect(() => {
        if (!pageVisible) return;
        if (!claudeDetected && !codexDetected) return;
        const timer = window.setInterval(() => {
            if (claudeDetected) void usageRefreshRef.current.claude();
            if (codexDetected) void usageRefreshRef.current.codex();
        }, USAGE_REFRESH_MS);
        return () => window.clearInterval(timer);
    }, [claudeDetected, codexDetected, pageVisible]);

    const isProject = session?.kind === "project";
    const cwd = session?.cwd ?? "";

    const selectedProvider = availableAgents.find((agent) => agent.type === selectedType);
    const selectedUsage = selectedType === "claude" ? claudeUsage : selectedType === "codex" ? codexUsage : null;
    const usagePeaks = {
        claude: usagePeak(claudeUsage.data),
        codex: usagePeak(codexUsage.data),
    };

    const projectCwds = useMemo(
        () =>
            sessionOrder
                .map((id) => sessions[id])
                .filter((entry) => entry?.kind === "project" && entry.cwd)
                .map((entry) => entry.cwd),
        [sessionOrder, sessions],
    );
    const openChats = useMemo(() => Object.values(agentsById).map((agent) => ({ agent: agent.type, id: persistedSessionIdOf(agent) })), [agentsById]);
    const recentProviders = useMemo(
        () => (allAgents ? availableAgents : availableAgents.filter((agent) => agent.type === selectedType)),
        [allAgents, availableAgents, selectedType],
    );
    const recentProjects = useMemo(() => (scope === "all" ? projectCwds : cwd ? [cwd] : []), [scope, projectCwds, cwd]);
    const needle = query.trim().toLowerCase();
    const recent = useRecentChats({
        enabled: isProject && recentProviders.length > 0 && recentProjects.length > 0,
        providers: recentProviders,
        projects: recentProjects,
        open: openChats,
        query: needle,
    });
    const { hasMore: hasMoreRecents, loadMore: loadMoreRecents } = recent;

    // Scrolling near the bottom asks for the next page. If a page does not
    // reach the bottom there is no scrollbar, so keep asking until the rail
    // fills, and check again when the rail is resized taller.
    useLayoutEffect(() => {
        const el = scrollRef.current;
        if (!el) return;
        const fill = () => {
            if (el.scrollHeight <= el.clientHeight && hasMoreRecents) loadMoreRecents();
        };
        fill();
        const ro = new ResizeObserver(fill);
        ro.observe(el);
        return () => ro.disconnect();
    }, [recent.chats.length, hasMoreRecents, loadMoreRecents]);

    const currentSession = useRef("");
    currentSession.current = session?.id ?? "";
    const closedRowBoxes = useRef(new Map<string, Box>());
    const leaveRow = useMemo(() => rowLeaving(currentSession, closedRowBoxes.current), []);
    const view = `${allAgents ? "all" : (selectedType ?? "")}:${scope}`;
    const seen = useRef<{ session?: string; active?: string; ids?: Set<string>; view?: string; stagger?: boolean }>({});
    /*
     * The rail's motion, read from what was just drawn: the selection glides
     * between open agents, a new agent opens its slot, and a provider switch
     * brings its name and recent chats in.
     */
    useLayoutEffect(() => {
        const scroll = scrollRef.current;
        const last = seen.current;
        const wraps = scroll ? [...scroll.querySelectorAll<HTMLElement>(".agent-row-wrap[data-agent-id]:not(.is-leaving)")] : [];
        const ids = new Set(wraps.map((w) => w.dataset.agentId ?? ""));
        const active = wraps.find((w) => w.querySelector(".agent-row.active"))?.dataset.agentId;
        const sameSession = last.session === session?.id;
        if (sameSession && last.ids) {
            for (const wrap of wraps) if (!last.ids.has(wrap.dataset.agentId ?? "")) arriveRow(wrap);
            if (active && last.active && active !== last.active) {
                const to = wraps.find((w) => w.dataset.agentId === active);
                const from = wraps.find((w) => w.dataset.agentId === last.active);
                const row = to?.querySelector<HTMLElement>(".agent-row");
                const group = to?.parentElement;
                const fromRow = from?.querySelector<HTMLElement>(".agent-row");
                const fromBox =
                    fromRow && group && from?.parentElement === group
                        ? contentBox(fromRow.getBoundingClientRect(), group)
                        : closedRowBoxes.current.get(last.active);
                if (row && group && fromBox) glideSelection(group, fromBox, row, fromRow);
            }
        }
        closedRowBoxes.current.clear();
        if (last.view !== undefined && last.view !== view) {
            animate(
                scroll?.parentElement?.querySelector(".agent-header-name"),
                [
                    { opacity: 0, transform: "translateY(3px)" },
                    { opacity: 1, transform: "none" },
                ],
                { duration: 140 },
            );
            last.stagger = true;
        }
        const recents = scroll ? [...scroll.querySelectorAll<HTMLElement>(".agent-row.recent")] : [];
        if (last.stagger && recents.length > 0) {
            recents.slice(0, 12).forEach((row, i) =>
                animate(
                    row,
                    [
                        { opacity: 0, transform: "translateY(4px)" },
                        { opacity: 1, transform: "none" },
                    ],
                    { duration: 150, delay: i * 20, fill: "backwards" },
                ),
            );
            last.stagger = false;
        }
        seen.current = { session: session?.id, active, ids, view, stagger: last.stagger };
    });

    if (!session) return null;

    const opens = sortByAttention(
        (
            agentIdsOf({ windowsBySession, windows: windowsById }, session.id)
                .map((id) => agentsById[id])
                .filter(Boolean) as Agent[]
        ).filter((a) => availableTypes.has(a.type)),
        activityById,
        backgroundById,
    );

    const menuAgent = menu ? agentsById[menu.agentId] : undefined;

    const onRailScroll = () => {
        if (!hasMoreRecents) return;
        const el = scrollRef.current;
        if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 120) loadMoreRecents();
    };

    const waitingElsewhere = agentsAwaitingInput({
        sessionOrder,
        sessions,
        windows: windowsById,
        windowsBySession,
        agents: agentsById,
        agentActivity,
    }).filter((entry) => entry.sessionId !== session.id).length;

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
                    plan={selectedUsage?.data?.plan}
                    canOpenPalette={catalogAgents.length > 0}
                    allAgents={allAgents}
                />
                <div className="agent-empty">agents are project-scoped</div>
                {!allAgents && isUsageAgent(selectedType) && selectedUsage && (
                    <AgentUsagePanel
                        provider={selectedType}
                        usage={selectedUsage}
                        label={availableAgents.find((a) => a.type === selectedType)?.label}
                        profiles={profiles}
                        selections={profileSelections}
                    />
                )}
            </>
        );
    }

    const noContent = opens.length === 0 && recent.chats.length === 0 && recent.status !== "loading";

    return (
        <>
            <AgentHeader
                agents={availableAgents}
                type={selectedType}
                setType={setType}
                searchOpen={searchOpen}
                onToggleSearch={toggleSearch}
                usagePeaks={usagePeaks}
                plan={selectedUsage?.data?.plan}
                canOpenPalette={catalogAgents.length > 0}
                allAgents={allAgents}
            />
            <ScopeTrack scope={scope} waitingElsewhere={waitingElsewhere} onChange={cmd.setAgentRailScope} />
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
                {scope === "all" && <AllProjectsAgents />}
                {scope === "project" && selectedType && (
                    <button
                        type="button"
                        className="agent-row agent-new"
                        onClick={() =>
                            allAgents
                                ? cmd.openAgentPalette()
                                : cmd.addAgent(selectedType, undefined, undefined, {
                                      profileId: selectedProviderProfile(selectedType, profiles, profileSelections)?.id,
                                      detectedExecutablePath: selectedProvider?.command,
                                  })
                        }>
                        <span className="agent-glyph">
                            <IconPlus size={15} />
                        </span>
                        <span className="agent-title">New chat</span>
                    </button>
                )}
                {scope === "project" && noContent && (
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

                {scope === "project" && opens.length > 0 && (
                    <Panel variant="group" className="agent-group">
                        <PanelHeader label="Open" rule />
                        {opens.map((a) => {
                            const active = activeAgentId({ windows: windowsById }, session) === a.id;
                            const glyph = (
                                <span className={`agent-glyph ${a.type}`}>
                                    <AgentIcon type={a.type} size={20} />
                                </span>
                            );
                            return (
                                <div key={a.id} className="agent-row-wrap" data-agent-id={a.id} data-session={session.id} ref={leaveRow}>
                                    {renamingId === a.id ? (
                                        <div className={`agent-row${active ? " active" : ""}`}>
                                            {glyph}
                                            <AgentTitleInput
                                                title={a.title}
                                                className="agent-title"
                                                onSave={(title) => cmd.renameAgent(a.id, title)}
                                                onDone={() => setRenamingId(null)}
                                            />
                                        </div>
                                    ) : (
                                        <button
                                            className={`agent-row${active ? " active" : ""}`}
                                            onClick={() => cmd.selectAgent(a.id)}
                                            onDoubleClick={() => setRenamingId(a.id)}
                                            onContextMenu={(event) => {
                                                event.preventDefault();
                                                setMenu({ agentId: a.id, x: event.clientX, y: event.clientY });
                                            }}>
                                            {glyph}
                                            <span className="agent-title">{a.title}</span>
                                        </button>
                                    )}
                                    <AgentStateMark state={activityById[a.id]?.state} background={(backgroundById[a.id] ?? 0) > 0} />
                                    <Tooltip label={`Close ${a.title}`}>
                                        <button type="button" className="row-x" aria-label={`Close ${a.title}`} onClick={() => cmd.closeAgent(a.id)}>
                                            <IconClose size={11} />
                                        </button>
                                    </Tooltip>
                                </div>
                            );
                        })}
                    </Panel>
                )}

                {menu && menuAgent && (
                    <AgentContextMenu
                        agent={menuAgent}
                        session={session}
                        x={menu.x}
                        y={menu.y}
                        onClose={() => setMenu(null)}
                        onRename={() => setRenamingId(menuAgent.id)}
                    />
                )}

                {scope === "project" && <AgentAttentionGroup />}

                <RecentChatList recent={recent} providers={availableAgents} />
            </div>
            {/* The rail's footer: plan limits sit under the agents they apply
                to, out of the way of the list you came here to use. */}
            {!allAgents && isUsageAgent(selectedType) && selectedUsage && (
                <AgentUsagePanel
                    provider={selectedType}
                    usage={selectedUsage}
                    label={availableAgents.find((a) => a.type === selectedType)?.label}
                    profiles={profiles}
                    selections={profileSelections}
                />
            )}
        </>
    );
}

/* Cross-project on purpose: an agent waiting on an answer is easy to miss in a
   project you are not currently looking at, which is the case this list is for. */
function AgentAttentionGroup() {
    /* Each slice is read on its own and the list derived in a memo: returning a
       fresh array straight from a store selector makes the snapshot differ on
       every render, which zustand answers with an endless re-render. */
    const sessionOrder = useStore((s) => s.sessionOrder);
    const sessions = useStore((s) => s.sessions);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agents = useStore((s) => s.agents);
    const agentActivity = useStore((s) => s.agentActivity);
    const activeSessionId = useStore((s) => s.activeSessionId);
    const waiting = useMemo(
        () => agentsAwaitingInput({ sessionOrder, sessions, windows, windowsBySession, agents, agentActivity }),
        [sessionOrder, sessions, windows, windowsBySession, agents, agentActivity],
    );
    if (waiting.length === 0) return null;
    return (
        <Panel variant="group" className="agent-group agent-attention">
            <PanelHeader label={waiting.length === 1 ? "1 action required" : `${waiting.length} actions required`} rule />
            {waiting.map((entry) => (
                <button
                    key={entry.agentId}
                    className="agent-row attention"
                    title={`${entry.sessionName} — ${entry.agentTitle} is waiting for you`}
                    onClick={() => cmd.revealAgent(entry.agentId)}>
                    <span className={`agent-glyph ${entry.agentType}`}>
                        <AgentIcon type={entry.agentType} size={20} />
                    </span>
                    <span className="agent-title">{entry.agentTitle}</span>
                    {entry.sessionId !== activeSessionId && <span className="agent-attention-project">{entry.sessionName}</span>}
                    <AgentStateIndicator state="blocked" />
                </button>
            ))}
        </Panel>
    );
}

function AgentStateMark({ state, background }: { state?: import("../state/types").AgentPresentationState; background: boolean }) {
    if (!state && !background) return null;
    return (
        <span className="row-status">
            <AgentStateIndicator state={state ?? "idle"} background={background} />
        </span>
    );
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

const MANAGE_ACCOUNTS = "manage-accounts";

function AccountPicker({
    provider,
    providerLabel,
    profiles,
    selections,
}: {
    provider: UsageAgentType;
    providerLabel: string;
    profiles: readonly ProviderProfile[];
    selections: ProviderProfileSelection;
}) {
    const accounts = useMemo(() => profiles.filter((profile) => profile.provider === provider), [profiles, provider]);
    if (accounts.length < 2) return null;
    const current = selectedProviderProfile(provider, profiles, selections) ?? accounts[0];
    return (
        <div className="agent-usage-account">
            <Dropdown
                label={`${providerLabel} account`}
                title={`New ${providerLabel} agents use this account`}
                value={current.id}
                align="right"
                menuWidth={180}
                options={[
                    ...accounts.map((profile) => ({ value: profile.id, label: profile.name, detail: profile.configPath })),
                    { value: MANAGE_ACCOUNTS, label: "Manage accounts…", className: "agent-usage-manage" },
                ]}
                onChange={(value) => {
                    if (value === MANAGE_ACCOUNTS) cmd.openSettings("agents");
                    else cmd.selectProviderProfile(provider, value);
                }}
            />
        </div>
    );
}

function AgentUsagePanel({
    provider,
    usage,
    label,
    profiles,
    selections,
}: {
    provider: UsageAgentType;
    usage: ResourceHandle<AgentUsage>;
    label?: string;
    profiles: readonly ProviderProfile[];
    selections: ProviderProfileSelection;
}) {
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
                <AccountPicker provider={provider} providerLabel={providerLabel} profiles={profiles} selections={selections} />
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
                                        <CountUp value={rounded} />
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
    plan,
    canOpenPalette,
    allAgents,
}: {
    agents: AgentInfo[];
    type: AgentType | null;
    setType: (t: AgentType) => void;
    searchOpen: boolean;
    onToggleSearch: () => void;
    usagePeaks: Partial<Record<UsageAgentType, number | undefined>>;
    plan?: string | null;
    canOpenPalette: boolean;
    allAgents: boolean;
}) {
    const chooseShortcut = useShortcutLabel("agent.choose");
    const label = agents.find((a) => a.type === type)?.label ?? type;
    return (
        <div className="agent-header">
            <div className="agent-header-top">
                {allAgents ? (
                    <span className="agent-header-name">
                        <span className="agent-glyph">
                            <IconInbox size={16} />
                        </span>
                        <span className="agent-header-label">All agents</span>
                    </span>
                ) : type ? (
                    <span className="agent-header-name">
                        <span className={`agent-glyph ${type}`}>
                            <AgentIcon type={type} size={16} />
                        </span>
                        <span className="agent-header-label">{label}</span>
                        {plan && <span className="agent-header-plan">{planLabel(plan)}</span>}
                    </span>
                ) : (
                    <span className="agent-header-label">Agents</span>
                )}
                <div className="agent-header-actions">
                    <Tooltip label="Filter recent chats">
                        <button className="agent-header-action" aria-pressed={searchOpen} aria-label="Filter recent chats" onClick={onToggleSearch}>
                            <IconSearch size={15} />
                        </button>
                    </Tooltip>
                    <Tooltip
                        label={type ? withShortcut("Choose agent", chooseShortcut) : canOpenPalette ? "Review agent setup" : "No agent CLI detected"}>
                        <button
                            className="agent-header-action"
                            disabled={!type && !canOpenPalette}
                            aria-label={type ? "Choose agent" : canOpenPalette ? "Review agent setup" : "No agent CLI detected"}
                            onClick={() => {
                                if (type || canOpenPalette) cmd.openAgentPalette();
                            }}>
                            <IconPlus size={15} />
                        </button>
                    </Tooltip>
                    <RailToggle edge="end" />
                </div>
            </div>
            <div className="agent-header-types" role="tablist" aria-label="Agent provider">
                {agents.length > 1 && (
                    <Tooltip label="All agents">
                        <button
                            role="tab"
                            aria-selected={allAgents}
                            tabIndex={allAgents ? 0 : -1}
                            onKeyDown={navigateTabs}
                            className={`agent-header-btn all-agents${allAgents ? " active" : ""}`}
                            aria-label="All agents"
                            onClick={(event) => {
                                event.currentTarget.focus();
                                cmd.setAgentRailAllAgents(true);
                            }}>
                            <IconInbox size={17} />
                        </button>
                    </Tooltip>
                )}
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
                            aria-selected={!allAgents && type === a.type}
                            tabIndex={!allAgents && type === a.type ? 0 : -1}
                            onKeyDown={navigateTabs}
                            className={`agent-header-btn ${a.type}${!allAgents && type === a.type ? " active" : ""}`}
                            aria-label={a.label}
                            onClick={(event) => {
                                event.currentTarget.focus();
                                setType(a.type);
                                cmd.setAgentRailAllAgents(false);
                            }}>
                            <AgentIcon type={a.type} size={18} />
                        </button>
                    </Tooltip>
                ))}
            </div>
        </div>
    );
}
