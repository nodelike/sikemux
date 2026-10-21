import { useLayoutEffect, useMemo, useRef, useState, type MouseEvent, type ReactNode, type Ref } from "react";
import { agentHandle } from "../agents/AgentLabel";
import { HostLabel } from "./RemoteRows";
import { AgentTitleInput } from "../agents/AgentTitleInput";
import { AgentStateIndicator, BackgroundCounts, showsAgentState } from "../agents/AgentStateIndicator";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { inTeams, type TeamLink } from "../agents/teams";
import type { AgentDisplay } from "../state/agentStatus";
import { activeAgentId, agentDisplayOf, hasChat, openAgents } from "../state/selectors";
import type { Agent } from "../state/types";
import { AgentFace } from "../buddies/Buddy";
import { IconAgent, IconClose, IconPin } from "../ui/Icons";
import { RailEmpty } from "./RailEmpty";
import { AgentContextMenu } from "../workspace/AgentContextMenu";
import { Panel } from "../ui/Panel";
import { Tooltip } from "../ui/Tooltip";
import { AgentRowProject, AgentRowTree } from "./AgentRowTree";
import { isRemoteWorker, RemoteAgentRow, teamWrapClass, withRemoteWorkers } from "./RemoteAgentRow";
import { drawTeamLines } from "./teams";
import { inCoreOrder, useInbox } from "./useInbox";

interface OpenAgentRow {
    id: string;
    lead?: string;
    agent: Agent;
    display: AgentDisplay;
    sessionName: string;
    sessionId: string;
    sessionCwd: string;
}

/** Every agent open in any project, in the order the core lists them. */
export function AllProjectsAgents({ showEmpty }: { showEmpty: boolean }) {
    const sessionOrder = useStore((s) => s.sessionOrder);
    const sessions = useStore((s) => s.sessions);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agents = useStore((s) => s.agents);
    const agentStatus = useStore((s) => s.agentStatus);
    const { inbox } = useInbox();
    const shownAgentId = useStore((s) => activeAgentId(s, s.sessions[s.activeSessionId]));
    const [renamingId, setRenamingId] = useState<string | null>(null);
    const [menu, setMenu] = useState<{ agentId: string; sessionId: string; x: number; y: number } | null>(null);

    const open = useMemo(() => {
        const rows: OpenAgentRow[] = [];
        for (const { agentId, agent, session } of openAgents({ sessionOrder, sessions, windows, windowsBySession, agents }))
            rows.push({
                id: agentId,
                lead: agent.lead,
                agent,
                display: agentDisplayOf({ agentStatus }, agentId),
                sessionName: session.name,
                sessionId: session.id,
                sessionCwd: session.cwd,
            });
        return rows;
    }, [sessionOrder, sessions, windows, windowsBySession, agents, agentStatus]);

    const teams = useMemo(
        () => inTeams(withRemoteWorkers(inCoreOrder(open, inbox.agents), inbox.remoteAgents)),
        [open, inbox.agents, inbox.remoteAgents],
    );

    const root = useRef<HTMLDivElement>(null);
    useLayoutEffect(() => {
        if (root.current) drawTeamLines(root.current);
    });

    const menuAgent = menu && agents[menu.agentId];
    const menuSession = menu && sessions[menu.sessionId];

    if (open.length === 0)
        return showEmpty && <RailEmpty icon={<IconAgent size={20} />} title="No agents open" shortcut="agent.new" doing="start one" />;

    return (
        <div ref={root} className="agent-all-projects">
            <Panel variant="group" className="agent-group">
                {teams.map(({ agent: row, link }) =>
                    isRemoteWorker(row) ? (
                        <RemoteAgentRow key={row.id} agent={row.remote} link={link} />
                    ) : (
                        <OpenAgentRow
                            key={row.id}
                            agent={row.agent}
                            display={row.display}
                            link={link}
                            active={row.id === shownAgentId}
                            project={{ name: row.sessionName, sessionId: row.sessionId }}
                            sessionCwd={row.sessionCwd}
                            title={
                                renamingId === row.id && (
                                    <AgentTitleInput
                                        title={row.agent.title}
                                        className="agent-title"
                                        onSave={(title) => cmd.renameAgent(row.id, title)}
                                        onDone={() => setRenamingId(null)}
                                    />
                                )
                            }
                            onDoubleClick={() => setRenamingId(row.id)}
                            onContextMenu={(event) => {
                                event.preventDefault();
                                setMenu({ agentId: row.id, sessionId: row.sessionId, x: event.clientX, y: event.clientY });
                            }}
                        />
                    ),
                )}
            </Panel>
            {menuAgent && menuSession && (
                <AgentContextMenu
                    agent={menuAgent}
                    session={menuSession}
                    x={menu.x}
                    y={menu.y}
                    onClose={() => setMenu(null)}
                    onRetitle={() => setRenamingId(menuAgent.id)}
                />
            )}
        </div>
    );
}

/** An open agent's row, with its state, its close, and what it is tied to hung below. `title` replaces its label while it is renamed. */
export function OpenAgentRow({
    agent,
    display,
    link,
    active,
    project,
    sessionCwd,
    hostShown = true,
    title,
    wrap,
    onOpen = () => cmd.revealAgent(agent.id),
    onDoubleClick,
    onContextMenu,
}: {
    agent: Agent;
    display: AgentDisplay;
    link?: TeamLink;
    active: boolean;
    project?: { name: string; sessionId: string };
    sessionCwd: string;
    /** Whether the row says which other host the agent runs on; a host's own list leaves it out. */
    hostShown?: boolean;
    title?: ReactNode;
    wrap?: { ref: Ref<HTMLDivElement>; "data-agent-id": string; "data-session": string };
    onOpen?: () => void;
    onDoubleClick?: () => void;
    onContextMenu?: (event: MouseEvent) => void;
}) {
    const glyph = <AgentFace agent={agent} size={18} logoSize={20} logoClass={`agent-glyph ${agent.type}`} buddyClass="agent-glyph buddy-mark" />;
    return (
        <div className={teamWrapClass(link)} {...wrap}>
            {title ? (
                <div className={`agent-row${active ? " active" : ""}`}>
                    {glyph}
                    {title}
                </div>
            ) : (
                <button
                    className={`agent-row${project ? " two-line" : ""}${active ? " active" : ""}`}
                    onClick={onOpen}
                    onDoubleClick={onDoubleClick}
                    onContextMenu={onContextMenu}>
                    {glyph}
                    <span className="agent-row-text">
                        <span className="agent-title">{agent.title}</span>
                        {project && <AgentRowProject name={project.name} />}
                    </span>
                    {hostShown && <HostLabel host={agent.host} />}
                    {agent.name && <span className="agent-row-name">{agentHandle(agent.name)}</span>}
                    <BackgroundCounts display={display} />
                </button>
            )}
            {showsAgentState(display) && (
                <span className="row-status">
                    <AgentStateIndicator display={display} />
                </span>
            )}
            {agent.resumeId && <PinToggle agent={agent} chatId={agent.resumeId} project={agent.cwd || sessionCwd} />}
            <Tooltip label={`Close ${agent.title}`}>
                <button type="button" className="row-x" aria-label={`Close ${agent.title}`} onClick={() => cmd.closeAgent(agent.id)}>
                    <IconClose size={11} />
                </button>
            </Tooltip>
            <AgentRowTree agentId={agent.id} worktree={agent.worktree} runsOn={link === "head" || link === "member"} />
        </div>
    );
}

function PinToggle({ agent, chatId, project }: { agent: Agent; chatId: string; project: string }) {
    const pinned = useStore((s) => s.pinnedChats.some((pin) => hasChat(agent, pin)));
    const label = `${pinned ? "Unpin" : "Pin"} ${agent.title}`;
    return (
        <Tooltip label={label}>
            <button
                type="button"
                className={`row-pin${pinned ? " pinned" : ""}`}
                aria-label={label}
                aria-pressed={pinned}
                onClick={() =>
                    pinned ? cmd.unpinChat(agent.type, chatId) : cmd.pinChat({ agent: agent.type, id: chatId, title: agent.title, project })
                }>
                <IconPin size={12} filled={pinned} />
            </button>
        </Tooltip>
    );
}
