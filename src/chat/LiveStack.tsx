import { useContext, type ReactNode } from "react";
import { PRIMARY_SHORTCUT } from "../lib/platform";
import { AgentIcon, IconClock, IconClose, IconCommand, IconTimer } from "../ui/Icons";
import { ChatAgentContext } from "./chatAgent";
import { queuedLabel, type QueuedMessage } from "./queuedMessages";
import { groupTasks, subagentActivity, taskDetail } from "./transcript";
import type { AcpAsyncTask, AcpSubagent } from "./types";

export function BackgroundTasks({ tasks, stopping, onStop }: { tasks: AcpAsyncTask[]; stopping: string[]; onStop: (taskId: string) => void }) {
    if (tasks.length === 0) return null;
    return (
        <>
            {groupTasks(tasks).map(([kind, group]) => (
                <Group label={kind} count={group.length} key={kind}>
                    {group.map((task) => (
                        <div className={`chat-task state-${task.state}`} key={task.asyncTaskId}>
                            {kind === "shell" ? <IconCommand size={12} /> : <IconTimer size={12} />}
                            <span className="chat-task-name">{task.name}</span>
                            <span className="chat-task-detail">{taskDetail(task)}</span>
                            {task.canStop && (
                                <button
                                    type="button"
                                    aria-label={`Stop ${task.name}`}
                                    disabled={stopping.includes(task.asyncTaskId)}
                                    onClick={() => onStop(task.asyncTaskId)}>
                                    <IconClose size={10} />
                                </button>
                            )}
                        </div>
                    ))}
                </Group>
            ))}
        </>
    );
}

/* A subagent at work belongs where the reader already watches for live things
   — the strip over the composer that the background tasks use. Its card in the
   transcript is where its output went, which is not where you look to find out
   whether it is still going. */
export function RunningSubagents({ subagents }: { subagents: AcpSubagent[] }) {
    const agentType = useContext(ChatAgentContext).type;
    if (subagents.length === 0) return null;
    return (
        <Group label="subagent" count={subagents.length}>
            {subagents.map((subagent) => (
                <div className="chat-task chat-task-agent" key={subagent.sessionId}>
                    <AgentIcon type={agentType} size={16} className={`agent-glyph ${agentType}`} />
                    <span className="chat-task-name">{subagent.name}</span>
                    <span className="chat-task-detail">{subagentActivity(subagent)}</span>
                    <span className="chat-task-spinner" aria-hidden="true" />
                </div>
            ))}
        </Group>
    );
}

/* One kind of running work, under a label that counts it. The label is what
   makes a stack of eight rows readable, so it stays even for a group of one.
   `plural` is for the kinds that are not a noun with an s on the end. */
function Group({
    label,
    plural,
    count,
    action,
    children,
}: {
    label: string;
    plural?: string;
    count: number;
    action?: ReactNode;
    children: ReactNode;
}) {
    const word = count === 1 ? label : (plural ?? `${label}s`);
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

export function QueuedMessages({
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
