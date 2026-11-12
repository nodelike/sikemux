import { useContext, type ReactNode } from "react";
import {
  IconCheck,
  IconChevron,
  IconClose,
  IconCommand,
  IconMessage,
  IconTimer,
  IconWorkflow,
} from "@shared/ui/Icons";
import { AgentMark } from "../agents/AgentMark";
import { ChatAgentContext } from "./chatAgent";
import { Shimmer } from "../ui/Shimmer";
import {
  groupTasks,
  subagentActivity,
  subagentCost,
  taskDetail,
  workflowAgentCost,
  workflowAgentCount,
  workflowAgentDetail,
} from "@shared/chat/transcript";
import type {
  AsyncTask,
  GuestMessage,
  SubagentCard,
  WorkflowAgent,
} from "@shared/wire.generated";

type GroupFolds = {
  folded: ReadonlySet<string>;
  toggle: (kind: string) => void;
};

function BackgroundTasks({
  tasks,
  folds,
}: {
  tasks: AsyncTask[];
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
          }}
        >
          {group.map((task) => [
            <div
              className={`chat-task state-${task.state}`}
              key={task.asyncTaskId}
            >
              <TaskIcon kind={kind} />
              <span className="chat-task-name">{task.name}</span>
              <span className="chat-task-detail">{taskDetail(task)}</span>
              {task.agents && task.agents.length > 0 && (
                <span className="chat-workflow-cost">
                  {workflowAgentCount(task.agents)}
                </span>
              )}
            </div>,
            ...(task.agents ?? []).map((agent) => (
              <WorkflowAgentRow
                agent={agent}
                key={`${task.asyncTaskId}-${agent.index}`}
              />
            )),
          ])}
        </Group>
      ))}
    </>
  );
}

function WorkflowAgentRow({ agent }: { agent: WorkflowAgent }) {
  const agentType = useContext(ChatAgentContext);
  const running = agent.state === "running";
  const detail = workflowAgentDetail(agent);
  const cost = workflowAgentCost(agent);
  return (
    <div
      className={`chat-task chat-task-agent chat-workflow-agent state-${agent.state}`}
    >
      <AgentMark type={agentType} size={16} />
      <span className="chat-task-name">{agent.label}</span>
      <span className={`chat-task-detail${running ? " chat-shimmer" : ""}`}>
        {running && detail ? <Shimmer>{detail}</Shimmer> : detail}
      </span>
      {cost && <span className="chat-workflow-cost">{cost}</span>}
      <span className="chat-task-end">
        {running && <span className="chat-task-spinner" aria-hidden="true" />}
        {agent.state === "completed" && (
          <IconCheck size={10} aria-label="done" />
        )}
        {agent.state === "failed" && (
          <IconClose size={10} aria-label="failed" />
        )}
      </span>
    </div>
  );
}

/* A subagent at work belongs where the reader already watches for live things
   — the strip over the composer that the background tasks use. Its card in the
   transcript is where its output went, which is not where you look to find out
   whether it is still going. */
function RunningSubagents({
  subagents,
  folds,
}: {
  subagents: SubagentCard[];
  folds: GroupFolds;
}) {
  const agentType = useContext(ChatAgentContext);
  if (subagents.length === 0) return null;
  const icon = <AgentMark type={agentType} size={16} />;
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
      }}
    >
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
   makes a stack of eight rows readable, so it stays even for a group of one. */
function Group({
  label,
  count,
  agent,
  fold,
  children,
}: {
  label: string;
  count: number;
  agent?: boolean;
  fold: Fold;
  children: ReactNode;
}) {
  const word = count === 1 ? label : `${label}s`;
  return (
    <div
      className={`chat-group${fold.folded ? " is-folded" : ""}${agent ? " chat-group-agent" : ""}`}
      aria-label={`${count} ${word}`}
    >
      <button
        type="button"
        className="chat-group-label chat-group-toggle"
        aria-expanded={!fold.folded}
        onClick={fold.onToggle}
      >
        {fold.folded && <span className="chat-group-icon">{fold.icon}</span>}
        <span>{word}</span>
        <span className="chat-group-count">{count}</span>
        {fold.folded && (
          <span className="chat-group-peek">{fold.peek.join(" · ")}</span>
        )}
        {fold.folded && fold.working && (
          <span className="chat-task-spinner" aria-hidden="true" />
        )}
        <span className="chat-group-chevron">
          <IconChevron size={10} />
        </span>
      </button>
      {!fold.folded && children}
    </div>
  );
}

/** Messages a shared chat's viewers wrote, waiting for the sharer to send or dismiss them. */
function WaitingGuests({
  guests,
  folds,
}: {
  guests: GuestMessage[];
  folds: GroupFolds;
}) {
  if (guests.length === 0) return null;
  return (
    <Group
      label="waiting message"
      count={guests.length}
      fold={{
        folded: folds.folded.has("guests"),
        onToggle: () => folds.toggle("guests"),
        icon: <IconMessage size={12} />,
        peek: guests.map((guest) => guest.text),
      }}
    >
      {guests.map((guest) => (
        <div className="chat-task" key={guest.id}>
          <IconMessage size={12} />
          <span className="chat-task-name">
            {guest.name ? `${guest.name} · guest` : "Guest"}
          </span>
          <span className="chat-task-detail">{guest.text}</span>
        </div>
      ))}
    </Group>
  );
}

/** What is still going on under the transcript: guests' waiting messages, running subagents and background tasks. */
export default function LiveStack({
  guests,
  subagents,
  tasks,
  folds,
}: {
  guests: GuestMessage[];
  subagents: SubagentCard[];
  tasks: AsyncTask[];
  folds: GroupFolds;
}) {
  return (
    <div className="chat-live-stack">
      <WaitingGuests guests={guests} folds={folds} />
      <RunningSubagents subagents={subagents} folds={folds} />
      <BackgroundTasks tasks={tasks} folds={folds} />
    </div>
  );
}
