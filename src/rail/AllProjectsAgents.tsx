import { useMemo, useState, type MouseEvent, type ReactNode, type Ref } from "react";
import { agentNaming } from "../agents/AgentLabel";
import { AgentTitleInput } from "../agents/AgentTitleInput";
import { AgentStateIndicator, BackgroundCounts, showsAgentState } from "../agents/AgentStateIndicator";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import type { AgentShown } from "@shared/wire.generated";
import { activeAgentId, hasChat, openAgents } from "../state/selectors";
import type { Agent } from "../state/types";
import { AgentFace } from "../buddies/Buddy";
import { IconAgent, IconChevron, IconClose, IconPin } from "../ui/Icons";
import { RailEmpty } from "./RailEmpty";
import { AgentContextMenu } from "../workspace/AgentContextMenu";
import { Panel } from "../ui/Panel";
import { Tooltip } from "../ui/Tooltip";
import { AgentRowProject } from "./AgentRowTree";
import { AgentRowCard, useRowCard } from "./AgentRowCard";
import { AgentReview } from "./AgentReview";
import { isRemoteWorker, RemoteAgentRow, teamWrapClass } from "./RemoteAgentRow";
import { coreTeams, foldTeams, useTeamFolds, type TeamFold, type TeamLink } from "./teams";
import { useInbox } from "./useInbox";
import { runningChats, usePlace } from "./HostLists";
import { hostName } from "../hosts/hosts";

interface OpenAgentRow {
    id: string;
    lead?: string;
    agent: Agent;
    display: AgentShown | undefined;
    sessionName: string;
    sessionId: string;
    sessionCwd: string;
}

/**
 * Every agent open in any project on `host` (none for this one), in the order the core lists them, then the chats
 * running there that this app does not show yet.
 */
export function AllProjectsAgents({ showEmpty, host }: { showEmpty: boolean; host: string | undefined }) {
    const sessionOrder = useStore((s) => s.sessionOrder);
    const sessions = useStore((s) => s.sessions);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agents = useStore((s) => s.agents);
    const agentStatus = useStore((s) => s.agentStatus);
    const { inbox } = useInbox();
    const place = usePlace();
    const where = useStore((s) => hostName(s, host));
    const shownAgentId = useStore((s) => activeAgentId(s, s.sessions[s.activeSessionId]));
    const [renamingId, setRenamingId] = useState<string | null>(null);
    const [menu, setMenu] = useState<{ agentId: string; sessionId: string; x: number; y: number } | null>(null);

    const open = useMemo(() => {
        const rows: OpenAgentRow[] = [];
        for (const { agentId, agent, session } of openAgents({ sessionOrder, sessions, windows, windowsBySession, agents })) {
            if (agent.host !== host) continue;
            rows.push({
                id: agentId,
                lead: agent.lead,
                agent,
                display: agentStatus[agentId],
                sessionName: session.name,
                sessionId: session.id,
                sessionCwd: session.cwd,
            });
        }
        return rows;
    }, [sessionOrder, sessions, windows, windowsBySession, agents, agentStatus, host]);

    const { folded, toggle } = useTeamFolds();
    const workers = useMemo(() => inbox.remoteAgents.filter((worker) => worker.hostId === host), [inbox.remoteAgents, host]);
    const teams = useMemo(() => foldTeams(coreTeams(open, inbox.agents, workers), folded, toggle), [open, inbox.agents, workers, folded, toggle]);
    const running = host ? runningChats(place, agents, host, new Set(workers.map((worker) => worker.agentId))) : [];

    const menuAgent = menu && agents[menu.agentId];
    const menuSession = menu && sessions[menu.sessionId];

    if (open.length + workers.length + running.length === 0)
        return (
            showEmpty && (
                <RailEmpty
                    icon={<IconAgent size={20} />}
                    title={where ? `No chats on ${where}` : "No agents open"}
                    shortcut="agent.new"
                    doing="start one"
                />
            )
        );

    return (
        <div className="agent-all-projects">
            <Panel variant="group" className="agent-group">
                {teams.map(({ agent: row, link, fold }) =>
                    isRemoteWorker(row) ? (
                        <RemoteAgentRow key={row.id} agent={row.remote} link={link} />
                    ) : (
                        <OpenAgentRow
                            key={row.id}
                            agent={row.agent}
                            display={row.display}
                            link={link}
                            fold={fold}
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
                {running}
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
    fold,
    active,
    project,
    sessionCwd,
    title,
    wrap,
    onOpen = () => cmd.revealAgent(agent.id),
    onDoubleClick,
    onContextMenu,
}: {
    agent: Agent;
    display: AgentShown | undefined;
    link?: TeamLink;
    /** On a lead's row, whether its team shows and how to fold it. */
    fold?: TeamFold;
    active: boolean;
    project?: { name: string; sessionId: string };
    sessionCwd: string;
    title?: ReactNode;
    wrap?: { ref: Ref<HTMLDivElement>; "data-agent-id": string; "data-session": string };
    onOpen?: () => void;
    onDoubleClick?: () => void;
    onContextMenu?: (event: MouseEvent) => void;
}) {
    const glyph = <AgentFace agent={agent} size={18} logoSize={20} logoClass={`agent-glyph ${agent.type}`} buddyClass="agent-glyph buddy-mark" />;
    const card = useRowCard();
    const profiles = useStore((s) => s.providerProfiles);
    const naming = agentNaming(agent, profiles);
    const [reviewAt, setReviewAt] = useState<DOMRect | null>(null);
    return (
        <div className={teamWrapClass(link, fold?.open)} {...wrap} onMouseEnter={card.enter} onMouseLeave={card.leave} onMouseDown={card.close}>
            {fold && (
                <button
                    type="button"
                    className="team-fold"
                    aria-expanded={fold.open}
                    aria-label={`${fold.open ? "Hide" : "Show"} the agents ${agent.title} started`}
                    onClick={fold.toggle}>
                    <IconChevron size={11} />
                </button>
            )}
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
                        <span className="agent-title">{naming.title}</span>
                        {project && <AgentRowProject name={project.name} />}
                    </span>
                    {naming.handle && <span className="agent-row-name">{naming.handle}</span>}
                    {fold && !fold.open && <span className="team-count">+{fold.count}</span>}
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
            {card.anchor && !title && (
                <AgentRowCard
                    agent={agent}
                    project={project?.name}
                    anchor={card.anchor}
                    onStay={card.stay}
                    onLeave={card.leave}
                    onReview={(at) => {
                        card.close();
                        setReviewAt(at);
                    }}
                />
            )}
            {reviewAt && agent.worktree && (
                <AgentReview agentId={agent.id} worktree={agent.worktree} anchor={reviewAt} onClose={() => setReviewAt(null)} />
            )}
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
