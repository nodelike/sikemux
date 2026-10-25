import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AgentInfo } from "../api/agents";
import { usePageVisible } from "../hooks/usePageVisible";
import { selectedAgentRuntimeProfiles, selectedProviderProfile } from "../agents/agentProfiles";
import { hasUsage, usageAgentTypes } from "@shared/agents/catalog";
import * as cmd from "../state/commands";
import { useShortcutLabel, withShortcut } from "../commands/useShortcutLabel";
import { useResource, useResourceEnabled, type ResourceHandle } from "../state/resources";
import { agentCatalogR, agentUsageR } from "../state/resources.defs";
import type { AgentUsage } from "../api/agents";
import { useStore } from "../state/store";
import { activeAgentId, agentIdsOf, chatKey, hasChat } from "../state/selectors";
import { type Agent, type AgentType, type PinnedChat } from "../state/types";
import { AgentIcon, IconAgent, IconDownload, IconGlobe, IconInbox, IconPin, IconPlus, IconSearch, IconTerminal } from "@shared/ui/Icons";
import { AgentTitleInput } from "../agents/AgentTitleInput";
import { AgentContextMenu } from "../workspace/AgentContextMenu";
import { Tooltip } from "../ui/Tooltip";
import { Dropdown } from "../ui/Dropdown";
import { Panel, PanelHeader } from "../ui/Panel";
import { TreeContextMenu } from "./FileTree";
import { animate, type Box, contentBox, EASE_LEAVE, glideSelection, leavingRef } from "../lib/motion";
import { leavingRail } from "./railMotion";
import { RailToggle } from "./RailToggle";
import { useSessionName } from "../terminal/railNames";
import { AgentAccountsPanel } from "./AgentAccounts";
import { RailEmpty } from "./RailEmpty";
import { AGENT_SETUP_SECTION } from "../settings/AgentSetupSection";
import { usagePeak } from "./usageFormat";
import { AllProjectsAgents, OpenAgentRow } from "./AllProjectsAgents";
import { RecentChatList } from "./RecentChatList";
import { ScopeTabs } from "./ScopeTabs";
import { NewRow } from "./RailNewRow";
import { RailPages } from "./RailPages";
import { FilesPage, TerminalsPage } from "./ProjectPages";
import { coreTeams, foldTeams, useTeamFolds, type TeamFold, type TeamLink } from "./teams";
import { useRecentChats } from "./useRecentChats";
import { inCoreOrder, itemSessionId, useInbox } from "./useInbox";
import { isRemoteWorker, RemoteAgentRow } from "./RemoteAgentRow";
import { NewOnHost, remoteChatRows, useHostProject } from "./RemoteRows";
import { hostName, openInstallPane } from "../hosts/hosts";
import { HostTerminals } from "./HostLists";
import { LEAVES_SETTINGS } from "../settings/leaveSettings";
import { preferencesOf } from "../state/preferences";

const USAGE_REFRESH_MS = 5 * 60_000;
const ALL_AGENTS = "all";

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

export const TabRail = memo(function TabRail() {
    const density = useStore((s) => preferencesOf(s).railDensity);
    return (
        <aside ref={leavingRail} className="workspace-rail tab-rail" aria-label="Agents" data-density={density} {...LEAVES_SETTINGS}>
            <RailPages pages={{ agents: <TabRailBody />, terminals: <TerminalsPage />, files: <FilesPage /> }} />
        </aside>
    );
});

export function TabRailBody() {
    const pageVisible = usePageVisible();
    const session = useStore((s) => s.sessions[s.activeSessionId]);
    const sessionName = useSessionName(session);
    const host = useStore((s) => s.chosenHost);
    const remoteHost = host.kind === "core" ? host.host : undefined;
    const local = host.kind === "core" && !remoteHost;
    const hostLabel = useStore((s) => hostName(s, remoteHost));
    const agentStatus = useStore((s) => s.agentStatus);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const windowsById = useStore((s) => s.windows);
    const agentsById = useStore((s) => s.agents);
    const place = useHostProject(session?.kind === "project" ? session : undefined);
    const pinnedChats = useStore((s) => s.pinnedChats);
    const profiles = useStore((s) => s.providerProfiles);
    const profileSelections = useStore((s) => s.selectedProviderProfileIds);
    const runtimeProfiles = useMemo(() => selectedAgentRuntimeProfiles(profiles, profileSelections), [profiles, profileSelections]);
    const catalog = useResource(agentCatalogR, runtimeProfiles);
    // Switching account re-detects the CLIs; the last answer holds the rail steady meanwhile.
    const [lastCatalog, setLastCatalog] = useState<AgentInfo[]>([]);
    if (catalog.data && catalog.data !== lastCatalog) setLastCatalog(catalog.data);
    const catalogAgents = catalog.data ?? lastCatalog;
    const availableAgents = useMemo(() => catalogAgents.filter((agent) => agent.available !== false), [catalogAgents]);
    const usages = useAgentUsages(availableAgents);
    const usageRefreshRef = useRef(usages);
    usageRefreshRef.current = usages;
    const usageDetected = usageAgentTypes().some((usageType) => usages[usageType].detected);

    const [type, setType] = useState<AgentType | null>(null);
    const allAgents = useStore((s) => s.tabRailAllAgents);
    const scope = useStore((s) => s.tabRailScope);
    const sessions = useStore((s) => s.sessions);
    const sessionOrder = useStore((s) => s.sessionOrder);
    const { inbox } = useInbox();
    // Recent chats live here and nowhere else, so the search for them does too.
    const [query, setQuery] = useState("");
    const [searchOpen, setSearchOpen] = useState(false);
    const [renamingId, setRenamingId] = useState<string | null>(null);
    const { folded, toggle: toggleTeam } = useTeamFolds();
    const [menu, setMenu] = useState<{ agentId: string; x: number; y: number } | null>(null);
    const [pinMenu, setPinMenu] = useState<{ pin: PinnedChat; x: number; y: number } | null>(null);
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
        if (!pageVisible || !usageDetected) return;
        const timer = window.setInterval(() => {
            for (const usage of Object.values(usageRefreshRef.current)) if (usage.detected) void usage.handle.refresh();
        }, USAGE_REFRESH_MS);
        return () => window.clearInterval(timer);
    }, [usageDetected, pageVisible]);

    const isProject = session?.kind === "project";
    const cwd = session?.cwd ?? "";

    const selectedProvider = availableAgents.find((agent) => agent.type === selectedType);
    const selectedUsage = hasUsage(selectedType) ? usages[selectedType].handle : null;
    const usagePeaks = Object.fromEntries(usageAgentTypes().map((usageType) => [usageType, usagePeak(usages[usageType].handle.data)]));

    const projectCwds = useMemo(
        () =>
            sessionOrder
                .map((id) => sessions[id])
                .filter((entry) => entry?.kind === "project" && entry.cwd)
                .map((entry) => entry.cwd),
        [sessionOrder, sessions],
    );
    const openChats = useMemo(
        () => [...Object.values(agentsById).map((agent) => ({ agent: agent.type, id: persistedSessionIdOf(agent) })), ...pinnedChats],
        [agentsById, pinnedChats],
    );
    const recentProviders = useMemo(
        () => (allAgents ? availableAgents : availableAgents.filter((agent) => agent.type === selectedType)),
        [allAgents, availableAgents, selectedType],
    );
    const recentProjects = useMemo(() => (scope === "all" ? projectCwds : cwd ? [cwd] : []), [scope, projectCwds, cwd]);
    const needle = query.trim().toLowerCase();
    const recent = useRecentChats({
        enabled: isProject && local && recentProviders.length > 0 && recentProjects.length > 0,
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
            if (scroll) holdList(scroll);
            last.stagger = true;
        }
        if (scroll && last.stagger && recent.status !== "loading") {
            revealList(scroll);
            last.stagger = false;
        }
        seen.current = { session: session?.id, active, ids, view, stagger: last.stagger };
    });

    const toggleSearch = () => {
        setQuery("");
        setSearchOpen((open) => !open);
    };

    const header = (
        <AgentHeader
            agents={availableAgents}
            type={selectedType}
            setType={setType}
            searchOpen={isProject && local && searchOpen}
            onToggleSearch={toggleSearch}
            usagePeaks={usagePeaks}
            canOpenPalette={catalogAgents.length > 0}
            allAgents={allAgents}
        />
    );
    // The rail's footer: plan limits sit under the agents they apply to, out of the way of the list you came here to use.
    const accounts = !allAgents && hasUsage(selectedType) && selectedUsage && <AgentAccountsPanel provider={selectedType} usage={selectedUsage} />;
    const actions = (
        <div className="agent-actions">
            <NewOnHost />
            {selectedType && (
                <NewRow
                    icon={<IconPlus size={15} />}
                    title="New chat"
                    shortcut="agent.new"
                    onClick={() =>
                        allAgents || scope === "all"
                            ? void cmd.startAgent()
                            : cmd.addAgent(selectedType, undefined, undefined, {
                                  profileId: selectedProviderProfile(selectedType, profiles, profileSelections)?.id,
                                  detectedExecutablePath: selectedProvider?.command,
                              })
                    }
                />
            )}
            <NewRow icon={<IconTerminal size={15} />} title="New terminal" shortcut="terminal.new" onClick={cmd.newTerminal} />
            <NewRow icon={<IconGlobe size={15} />} title="New browser" shortcut="browser.tabNew" onClick={cmd.newDeskBrowserTab} />
        </div>
    );

    if (host.kind === "ssh") {
        return (
            <>
                {header}
                <div className="tab-rail-top">
                    <div className="agent-actions">
                        <NewOnHost />
                        <NewRow icon={<IconTerminal size={15} />} title="New terminal" shortcut="terminal.new" onClick={cmd.newTerminal} />
                        <NewRow
                            icon={<IconDownload size={15} />}
                            title="Install Sikemux"
                            shortcut="agent.new"
                            onClick={() => openInstallPane(host.alias)}
                        />
                    </div>
                    {session?.kind === "project" && <ScopeTabs scope={scope} name={session.name} onChange={cmd.setTabRailScope} />}
                </div>
                <div className="rail-scroll">
                    <HostTerminals choice={host} />
                </div>
                {accounts}
            </>
        );
    }

    if (!session) return null;

    const sessionAgents = agentIdsOf({ windowsBySession, windows: windowsById }, session.id)
        .map((id) => agentsById[id])
        .filter((agent): agent is Agent => agent?.host === remoteHost);
    const pins = local
        ? pinnedChats.filter((pin) => pin.project === session.cwd).map((pin) => ({ pin, agent: sessionAgents.find((a) => hasChat(a, pin)) }))
        : [];
    const workers = inbox.remoteAgents.filter((worker) => worker.hostId === remoteHost);
    const opens = inCoreOrder(
        sessionAgents.filter((a) => !pins.some(({ pin }) => hasChat(a, pin))),
        inbox.agents,
    );

    const menuAgent = menu ? agentsById[menu.agentId] : undefined;

    const openRow = (a: Agent, link?: TeamLink, fold?: TeamFold) => (
        <OpenAgentRow
            key={a.id}
            agent={a}
            display={agentStatus[a.id]}
            link={link}
            fold={fold}
            active={activeAgentId({ windows: windowsById }, session) === a.id}
            sessionCwd={session.cwd}
            wrap={{ ref: leaveRow, "data-agent-id": a.id, "data-session": session.id }}
            title={
                renamingId === a.id && (
                    <AgentTitleInput
                        title={a.title}
                        className="agent-title"
                        onSave={(title) => cmd.renameAgent(a.id, title)}
                        onDone={() => setRenamingId(null)}
                    />
                )
            }
            onDoubleClick={() => setRenamingId(a.id)}
            onContextMenu={(event) => {
                event.preventDefault();
                setMenu({ agentId: a.id, x: event.clientX, y: event.clientY });
            }}
        />
    );

    const onRailScroll = () => {
        if (!hasMoreRecents) return;
        const el = scrollRef.current;
        if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 120) loadMoreRecents();
    };

    const waitingElsewhere = inbox.items.filter(
        (item) => item.tier === "needs" && itemSessionId({ sessionOrder, sessions, windows: windowsById, windowsBySession }, item) !== session.id,
    ).length;

    if (!isProject) {
        return (
            <>
                {header}
                <RailEmpty icon={<IconAgent size={20} />} title="No project open" shortcut="project.open" doing="open one" />
                {accounts}
            </>
        );
    }

    const remoteChats = remoteChatRows({
        sessionId: session.id,
        place: place?.host.id === remoteHost ? place : undefined,
        open: new Set([...Object.keys(agentsById), ...workers.map((worker) => worker.agentId)]),
    });
    const noRecents = recent.chats.length === 0 && recent.status !== "loading";
    const noContent = pins.length === 0 && opens.length === 0 && workers.length === 0 && remoteChats.length === 0 && noRecents;

    return (
        <>
            {header}
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
            <div className="tab-rail-top">
                {actions}
                <ScopeTabs scope={scope} name={sessionName} waitingElsewhere={waitingElsewhere} onChange={cmd.setTabRailScope} />
            </div>
            <div className="rail-scroll" ref={scrollRef} onScroll={onRailScroll}>
                {scope === "all" && <AllProjectsAgents showEmpty={noRecents} host={remoteHost} />}
                {scope === "project" && noContent && (
                    <RailEmpty
                        icon={catalog.status === "loading" || !needle ? <IconAgent size={20} /> : <IconSearch size={20} />}
                        {...(hostLabel
                            ? { title: `No chats in ${session.name} on ${hostLabel}`, shortcut: "agent.new" as const, doing: "start one" }
                            : catalog.status === "loading"
                              ? { title: "Looking for agents…" }
                              : availableAgents.length === 0
                                ? {
                                      title: "No agents set up",
                                      action: { label: "Set one up", onClick: () => cmd.openSettings("agents", AGENT_SETUP_SECTION) },
                                  }
                                : needle
                                  ? { title: "No chats match", message: "Try another filter" }
                                  : { title: "No agents yet", shortcut: "agent.new" as const, doing: "start one" })}
                    />
                )}

                {scope === "project" && (opens.length > 0 || workers.length > 0 || remoteChats.length > 0) && (
                    <Panel variant="group" className="agent-group">
                        {foldTeams(coreTeams(opens, inbox.agents, workers), folded, toggleTeam).map(({ agent: a, link, fold }) =>
                            isRemoteWorker(a) ? <RemoteAgentRow key={a.id} agent={a.remote} link={link} /> : openRow(a, link, fold),
                        )}
                        {remoteChats}
                    </Panel>
                )}

                {scope === "project" && pins.length > 0 && (
                    <Panel variant="group" className="agent-group">
                        <PanelHeader label="Pinned" rule />
                        {pins.map(({ pin, agent: a }) =>
                            a ? (
                                openRow(a)
                            ) : (
                                <div key={chatKey(pin.agent, pin.id)} className="agent-recent">
                                    <button
                                        className="agent-row recent"
                                        onClick={() =>
                                            cmd.openSavedChat(pin, {
                                                detectedExecutablePath: availableAgents.find((agent) => agent.type === pin.agent)?.command,
                                            })
                                        }
                                        onContextMenu={(event) => {
                                            event.preventDefault();
                                            setPinMenu({ pin, x: event.clientX, y: event.clientY });
                                        }}>
                                        <span className={`agent-glyph ${pin.agent}`}>
                                            <AgentIcon type={pin.agent} size={20} />
                                        </span>
                                        <span className="agent-row-text">
                                            <span className="agent-title">{pin.title}</span>
                                        </span>
                                        <span className="agent-ago">
                                            <IconPin size={11} filled />
                                        </span>
                                    </button>
                                </div>
                            ),
                        )}
                    </Panel>
                )}

                {pinMenu && (
                    <TreeContextMenu
                        x={pinMenu.x}
                        y={pinMenu.y}
                        items={[{ label: "Unpin", icon: <IconPin size={13} />, run: () => cmd.unpinChat(pinMenu.pin.agent, pinMenu.pin.id) }]}
                        onClose={() => setPinMenu(null)}
                    />
                )}

                {menu && menuAgent && (
                    <AgentContextMenu
                        agent={menuAgent}
                        session={session}
                        x={menu.x}
                        y={menu.y}
                        onClose={() => setMenu(null)}
                        onRetitle={() => setRenamingId(menuAgent.id)}
                    />
                )}

                {local && <RecentChatList recent={recent} providers={availableAgents} />}
            </div>
            {accounts}
        </>
    );
}

/** How much of each plan is used, for the agents found on this machine. */
function useAgentUsages(available: readonly AgentInfo[]): Record<AgentType, { detected: boolean; handle: ResourceHandle<AgentUsage> }> {
    return Object.fromEntries(
        usageAgentTypes().map((type) => {
            const provider = available.find((agent) => agent.type === type);
            // The app reads the catalog once, before it first draws, so these hooks run in the same order every render.
            // eslint-disable-next-line react-hooks/rules-of-hooks
            const handle = useResourceEnabled(!!provider, agentUsageR, type, provider?.configPath ?? undefined);
            return [type, { detected: !!provider, handle }];
        }),
    ) as Record<AgentType, { detected: boolean; handle: ResourceHandle<AgentUsage> }>;
}

function AgentHeader({
    agents,
    type,
    setType,
    searchOpen,
    onToggleSearch,
    usagePeaks,
    canOpenPalette,
    allAgents,
}: {
    agents: AgentInfo[];
    type: AgentType | null;
    setType: (t: AgentType) => void;
    searchOpen: boolean;
    onToggleSearch: () => void;
    usagePeaks: Partial<Record<AgentType, number | undefined>>;
    canOpenPalette: boolean;
    allAgents: boolean;
}) {
    const chooseShortcut = useShortcutLabel("agent.choose");
    const label = agents.find((a) => a.type === type)?.label ?? type;
    return (
        <div className="agent-header">
            <div className="agent-header-top">
                {agents.length > 1 ? (
                    <Dropdown
                        className="agent-picker"
                        label="Agent provider"
                        value={allAgents ? ALL_AGENTS : (type ?? "")}
                        icon={
                            allAgents ? (
                                <span className="agent-glyph">
                                    <IconInbox size={16} />
                                </span>
                            ) : (
                                type && (
                                    <span className={`agent-glyph ${type}`}>
                                        <AgentIcon type={type} size={16} />
                                    </span>
                                )
                            )
                        }
                        options={[
                            {
                                value: ALL_AGENTS,
                                label: "All harnesses",
                                icon: (
                                    <span className="agent-glyph">
                                        <IconInbox size={14} />
                                    </span>
                                ),
                            },
                            ...agents.map((a) => ({
                                value: a.type,
                                label: a.label,
                                icon: (
                                    <span className={`agent-glyph ${a.type}`}>
                                        <AgentIcon type={a.type} size={14} />
                                    </span>
                                ),
                                meta: hasUsage(a.type) && usagePeaks[a.type] != null ? `${Math.round(usagePeaks[a.type]!)}%` : undefined,
                            })),
                        ]}
                        menuWidth={200}
                        checkAtEnd
                        onChange={(value) => {
                            if (value === ALL_AGENTS) return cmd.setTabRailAllAgents(true);
                            setType(value);
                            cmd.setTabRailAllAgents(false);
                        }}
                    />
                ) : type ? (
                    <span className="agent-header-name">
                        <span className={`agent-glyph ${type}`}>
                            <AgentIcon type={type} size={16} />
                        </span>
                        <span className="agent-header-label">{label}</span>
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
        </div>
    );
}

const LIST_HOLD_MS = 300;
const listHolds = new WeakMap<HTMLElement, number>();

/* A new view's list waits hidden for its chats, so it never shows half of its rows. */
function holdList(scroll: HTMLElement): void {
    scroll.dataset.revealing = "true";
    window.clearTimeout(listHolds.get(scroll));
    listHolds.set(
        scroll,
        window.setTimeout(() => revealList(scroll), LIST_HOLD_MS),
    );
}

/* Then the whole list wipes in from the top as one piece. */
function revealList(scroll: HTMLElement): void {
    window.clearTimeout(listHolds.get(scroll));
    if (scroll.dataset.revealing !== "true") return;
    delete scroll.dataset.revealing;
    animate(
        scroll,
        [
            { clipPath: "inset(0 0 100% 0)", opacity: 0.4 },
            { clipPath: "inset(0 0 0% 0)", opacity: 1 },
        ],
        { duration: 200 },
    );
}
