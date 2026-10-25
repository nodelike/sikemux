import { useContext, type ReactNode } from "react";
import { PRIMARY_SHORTCUT } from "@shared/lib/platform";
import { AgentIcon, IconCheck, IconChevron, IconClock, IconClose, IconCommand, IconTimer, IconWorkflow } from "../ui/Icons";
import { ChatAgentContext } from "./chatAgent";
import { queuedLabel } from "@shared/chat/queuedMessages";
import type { QueuedMessage } from "@shared/chat/types";
import { Shimmer } from "./Shimmer";
import {
    groupTasks,
    subagentActivity,
    subagentCost,
    taskDetail,
    workflowAgentCost,
    workflowAgentCount,
    workflowAgentDetail,
} from "@shared/chat/transcript";
import type { AsyncTask, SubagentCard, WorkflowAgent } from "@shared/chat/types";

export type GroupFolds = {
    folded: ReadonlySet<string>;
    toggle: (kind: string) => void;
};

function BackgroundTasks({
    tasks,
    stopping,
    onStop,
    folds,
}: {
    tasks: AsyncTask[];
    stopping: string[];
    /** Without it, as in a shared chat, a task cannot be stopped from here. */
    onStop?: (taskId: string) => void;
    folds: GroupFolds;
}) {
    if (tasks.length === 0) return null;
    return (
        <>
            {groupTasks(tasks).map(([kind, group]) => (
                <Group
                    label={kind}
                    count={group.length}
                    key={kind}
                    fold={{
                        folded: folds.folded.has(kind),
                        onToggle: () => folds.toggle(kind),
                        icon: <TaskIcon kind={kind} />,
                        peek: group.map((task) => task.name),
                    }}>
                    {group.map((task) => [
                        <div className={`chat-task state-${task.state}`} key={task.asyncTaskId}>
                            <TaskIcon kind={kind} />
                            <span className="chat-task-name">{task.name}</span>
                            <span className="chat-task-detail">{taskDetail(task)}</span>
                            {task.agents && task.agents.length > 0 && <span className="chat-workflow-cost">{workflowAgentCount(task.agents)}</span>}
                            {task.canStop && onStop && (
                                <button
                                    type="button"
                                    aria-label={`Stop ${task.name}`}
                                    disabled={stopping.includes(task.asyncTaskId)}
                                    onClick={() => onStop(task.asyncTaskId)}>
                                    <IconClose size={10} />
                                </button>
                            )}
                        </div>,
                        ...(task.agents ?? []).map((agent) => <WorkflowAgentRow agent={agent} key={`${task.asyncTaskId}-${agent.index}`} />),
                    ])}
                </Group>
            ))}
        </>
    );
}

function WorkflowAgentRow({ agent }: { agent: WorkflowAgent }) {
    const agentType = useContext(ChatAgentContext).type;
    const running = agent.state === "running";
    const detail = workflowAgentDetail(agent);
    const cost = workflowAgentCost(agent);
    return (
        <div className={`chat-task chat-task-agent chat-workflow-agent state-${agent.state}`}>
            <AgentIcon type={agentType} size={16} className={`agent-glyph ${agentType}`} />
            <span className="chat-task-name">{agent.label}</span>
            <span className={`chat-task-detail${running ? " chat-shimmer" : ""}`}>{running && detail ? <Shimmer>{detail}</Shimmer> : detail}</span>
            {cost && <span className="chat-workflow-cost">{cost}</span>}
            <span className="chat-task-end">
                {running && <span className="chat-task-spinner" aria-hidden="true" />}
                {agent.state === "completed" && <IconCheck size={10} aria-label="done" />}
                {agent.state === "failed" && <IconClose size={10} aria-label="failed" />}
            </span>
        </div>
    );
}

/* A subagent at work belongs where the reader already watches for live things
   — the strip over the composer that the background tasks use. Its card in the
   transcript is where its output went, which is not where you look to find out
   whether it is still going. */
function RunningSubagents({ subagents, folds }: { subagents: SubagentCard[]; folds: GroupFolds }) {
    const agentType = useContext(ChatAgentContext).type;
    if (subagents.length === 0) return null;
    const icon = <AgentIcon type={agentType} size={16} className={`agent-glyph ${agentType}`} />;
    return (
        <Group
            label="subagent"
            count={subagents.length}
            agent
            fold={{
                folded: folds.folded.has("subagent"),
                onToggle: () => folds.toggle("subagent"),
                icon,
                peek: subagents.map((subagent) => subagent.name),
                working: true,
            }}>
            {subagents.map((subagent) => {
                const cost = subagentCost(subagent);
                return (
                    <div className="chat-task chat-task-agent" key={subagent.sessionId}>
                        {icon}
                        <span className="chat-task-name">{subagent.name}</span>
                        <span className="chat-task-detail chat-shimmer">
                            <Shimmer>{subagentActivity(subagent)}</Shimmer>
                        </span>
                        {cost && <span className="chat-workflow-cost">{cost}</span>}
                        <span className="chat-task-end">
                            <span className="chat-task-spinner" aria-hidden="true" />
                        </span>
                    </div>
                );
            })}
        </Group>
    );
}

function TaskIcon({ kind }: { kind: string }) {
    if (kind === "shell") return <IconCommand size={12} />;
    if (kind === "workflow") return <IconWorkflow size={12} />;
    return <IconTimer size={12} />;
}

type Fold = {
    folded: boolean;
    onToggle: () => void;
    icon: ReactNode;
    peek: string[];
    working?: boolean;
};

/* One kind of running work, under a label that counts it. The label is what
   makes a stack of eight rows readable, so it stays even for a group of one.
   `plural` is for the kinds that are not a noun with an s on the end. */
function Group({
    label,
    plural,
    count,
    action,
    agent,
    fold,
    children,
}: {
    label: string;
    plural?: string;
    count: number;
    action?: ReactNode;
    agent?: boolean;
    fold?: Fold;
    children: ReactNode;
}) {
    const word = count === 1 ? label : (plural ?? `${label}s`);
    if (!fold) {
        return (
            <div className="chat-group" aria-label={`${count} ${word}`}>
                <div className="chat-group-label">
                    <span>{word}</span>
                    <span className="chat-group-count">{count}</span>
                    {action}
                </div>
                {children}
            </div>
        );
    }
    return (
        <div className={`chat-group${fold.folded ? " is-folded" : ""}${agent ? " chat-group-agent" : ""}`} aria-label={`${count} ${word}`}>
            <button type="button" className="chat-group-label chat-group-toggle" aria-expanded={!fold.folded} onClick={fold.onToggle}>
                {fold.folded && <span className="chat-group-icon">{fold.icon}</span>}
                <span>{word}</span>
                <span className="chat-group-count">{count}</span>
                {fold.folded && <span className="chat-group-peek">{fold.peek.join(" · ")}</span>}
                {fold.folded && fold.working && <span className="chat-task-spinner" aria-hidden="true" />}
                <span className="chat-group-chevron">
                    <IconChevron size={10} />
                </span>
            </button>
            {!fold.folded && children}
        </div>
    );
}

function QueuedMessages({
    messages,
    steerable,
    onSteer,
    onDrop,
}: {
    messages: QueuedMessage[];
    steerable: boolean;
    onSteer: (messages: QueuedMessage[]) => void;
    onDrop: (id: string) => void;
}) {
    if (messages.length === 0) return null;
    const steerAll = steerable && messages.length > 1 && (
        <button
            type="button"
            className="chat-queued-steer chat-queued-steer-all"
            aria-label="Steer the running turn with every queued message"
            onClick={() => onSteer(messages)}>
            Steer all
            <kbd className="chat-queued-steer-key">{PRIMARY_SHORTCUT}↵</kbd>
        </button>
    );
    return (
        <Group label="queued" plural="queued" count={messages.length} action={steerAll}>
            {messages.map((message) => {
                const label = queuedLabel(message);
                return (
                    <div className="chat-queued-message" key={message.id}>
                        <IconClock size={12} />
                        <span className="chat-queued-text">{label}</span>
                        {steerable && (
                            <button
                                type="button"
                                className="chat-queued-steer"
                                aria-label={`Steer the running turn with ${label}`}
                                onClick={() => onSteer([message])}>
                                Steer
                                {messages.length === 1 && <kbd className="chat-queued-steer-key">{PRIMARY_SHORTCUT}↵</kbd>}
                            </button>
                        )}
                        <button type="button" aria-label={`Drop ${label} from the queue`} onClick={() => onDrop(message.id)}>
                            <IconClose size={10} />
                        </button>
                    </div>
                );
            })}
        </Group>
    );
}

/** What is still going on under the transcript: questions waiting on the person, running subagents, background tasks and messages waiting their turn. */
export default function LiveStack({
    questions,
    subagents,
    tasks,
    folds,
    stopping = [],
    onStop,
    queue,
}: {
    questions?: ReactNode;
    subagents: SubagentCard[];
    tasks: AsyncTask[];
    folds: GroupFolds;
    stopping?: string[];
    onStop?: (taskId: string) => void;
    /** Without it, as in a shared chat, nothing can be queued here. */
    queue?: { messages: QueuedMessage[]; steerable: boolean; onSteer: (messages: QueuedMessage[]) => void; onDrop: (id: string) => void };
}) {
    return (
        <div className="chat-live-stack">
            {questions}
            <RunningSubagents subagents={subagents} folds={folds} />
            <BackgroundTasks tasks={tasks} stopping={stopping} onStop={onStop} folds={folds} />
            {queue && <QueuedMessages {...queue} />}
        </div>
    );
}
