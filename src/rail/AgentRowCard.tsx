import { useEffect, useRef, useState, type MouseEvent, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { agentHandle } from "../agents/AgentLabel";
import { agentLabel } from "../agents/catalog";
import { AgentFace } from "../buddies/Buddy";
import { useBranchPullList, useHostRepo } from "../codehost/project";
import { prettyPath } from "../lib/paths";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import type { Agent, AgentWorktree } from "../state/types";
import { IconFolder, IconGit, IconPullRequest } from "../ui/Icons";

const OPEN_AFTER_MS = 450;
const CLOSE_AFTER_MS = 140;

/** A card that opens after the pointer rests on a row, and stays while the pointer moves onto it. */
export function useRowCard() {
    const [anchor, setAnchor] = useState<DOMRect | null>(null);
    const timer = useRef<number | undefined>(undefined);
    useEffect(() => () => window.clearTimeout(timer.current), []);
    const enter = (event: MouseEvent<HTMLElement>) => {
        const row = event.currentTarget;
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setAnchor(row.getBoundingClientRect()), anchor ? 0 : OPEN_AFTER_MS);
    };
    const stay = () => window.clearTimeout(timer.current);
    const leave = () => {
        window.clearTimeout(timer.current);
        timer.current = window.setTimeout(() => setAnchor(null), CLOSE_AFTER_MS);
    };
    const close = () => {
        window.clearTimeout(timer.current);
        setAnchor(null);
    };
    return { anchor, enter, stay, leave, close };
}

/** What an agent row stands for, beside the row: its project, branch, folder and pull requests. */
export function AgentRowCard({
    agent,
    project,
    anchor,
    onStay,
    onLeave,
    onReview,
}: {
    agent: Agent;
    project?: string;
    anchor: DOMRect;
    onStay: () => void;
    onLeave: () => void;
    onReview: (at: DOMRect) => void;
}) {
    const home = useStore((s) => s.home);
    const onLeftHalf = anchor.left < window.innerWidth / 2;
    const top = Math.max(8, Math.min(anchor.top - 6, window.innerHeight - 220));
    const side = onLeftHalf ? { left: anchor.right + 8 } : { right: window.innerWidth - anchor.left + 8 };
    return createPortal(
        <div className="agent-card" style={{ top, ...side }} onMouseEnter={onStay} onMouseLeave={onLeave}>
            <div className="agent-card-head">
                <AgentFace agent={agent} size={22} logoSize={22} logoClass={`agent-glyph ${agent.type}`} buddyClass="agent-glyph buddy-mark" />
                <span className="agent-card-who">
                    <b>{agent.title}</b>
                    <span>{[agent.name && agentHandle(agent.name), agentLabel(agent.type)].filter(Boolean).join(" · ")}</span>
                </span>
            </div>
            {(project || agent.worktree) && (
                <div className="agent-card-facts">
                    {project && (
                        <CardFact icon={<IconFolder size={13} />}>
                            <span>{project}</span>
                        </CardFact>
                    )}
                    {agent.worktree && (
                        <>
                            <CardFact icon={<IconGit size={13} />}>
                                <button
                                    type="button"
                                    className="agent-card-link mono"
                                    title={`Review ${agent.worktree.branch}`}
                                    onClick={(event) => onReview(event.currentTarget.getBoundingClientRect())}>
                                    {agent.worktree.branch}
                                </button>
                            </CardFact>
                            <CardFact icon={<span className="agent-card-gap" />}>
                                <span className="mono quiet" title={agent.worktree.path}>
                                    {prettyPath(agent.worktree.path, home)}
                                </span>
                            </CardFact>
                            <CardPulls agentId={agent.id} worktree={agent.worktree} />
                        </>
                    )}
                </div>
            )}
        </div>,
        document.body,
    );
}

function CardFact({ icon, children }: { icon: ReactNode; children: ReactNode }) {
    return (
        <div className="agent-card-fact">
            <span className="agent-card-icon">{icon}</span>
            {children}
        </div>
    );
}

function CardPulls({ agentId, worktree }: { agentId: string; worktree: AgentWorktree }) {
    const host = useHostRepo(worktree.path, true);
    const pulls = useBranchPullList(host.repo ? worktree.path : null, worktree.branch, true);
    if (pulls.length === 0) return null;
    return (
        <CardFact icon={<IconPullRequest size={13} />}>
            <span className="agent-card-pulls">
                {pulls.map((pull) => (
                    <button
                        key={pull.number}
                        type="button"
                        className="agent-card-link mono"
                        title={pull.title}
                        onClick={() => cmd.openUrlOnDesk(agentId, pull.url)}>
                        #{pull.number}
                    </button>
                ))}
            </span>
        </CardFact>
    );
}
