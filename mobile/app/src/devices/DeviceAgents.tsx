import { Fragment, memo } from 'react';
import { StyleSheet } from 'react-native';
import { router } from 'expo-router';

import type { AgentShown, ProjectInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { WorkerRow, workersOf } from '@/devices/RemoteWorker';
import { useMinute } from '@/ui/motion';
import { AgentFace } from '@/devices/Buddy';
import { Row, Rows } from '@/ui/list';
import { EmptyState } from '@/ui/prompt';
import { AgentState, ShellCount, SubagentCount, TriggerTime } from '@/devices/AgentMarks';
import { useStyles } from '@/ui/theme';
import { listed, Recent } from '@/devices/DeviceRecent';

export type Tab = 'agents' | 'terminals' | 'board';

/** The open agents in the order the host lists them on every device, what wants the person first. */
function openAgents(snapshot: Snapshot, scope: ProjectInfo | undefined, provider: string): AgentShown[] {
  return snapshot.inbox.agents.filter((agent) => listed(scope, provider, agent.project, agent.cli));
}

/** The host's agents: its open agents in one list, each with its own state, then its recent chats. */
export function Agents({
  core,
  snapshot,
  scope,
  provider,
  full,
  hostName,
  onTab,
}: {
  core: string;
  snapshot: Snapshot;
  scope?: ProjectInfo;
  provider: string;
  full: boolean;
  hostName: string;
  onTab: (tab: Tab) => void;
}) {
  const styles = useStyles(makeStyles);
  const minute = useMinute();
  const open = openAgents(snapshot, scope, provider);
  const workers = snapshot.inbox.remoteAgents;
  const leads = new Set(open.map((agent) => agent.agentId));
  const strays = scope
    ? []
    : workers.filter((worker) => !(worker.lead && leads.has(worker.lead)) && listed(undefined, provider, undefined, worker.cli));
  const placeOf = (agent: AgentShown) => (scope ? undefined : (agent.projectName ?? undefined));

  return (
    <>
      {open.length || strays.length ? (
        <Rows style={styles.open}>
          {open.map((agent) => (
            <Fragment key={agent.agentId}>
              {agent.session == null ? (
                <ChatRow core={core} shown={agent} place={placeOf(agent)} minute={agent.nextTrigger == null ? 0 : minute} />
              ) : (
                <TerminalAgentRow
                  shown={agent}
                  place={placeOf(agent)}
                  minute={agent.nextTrigger == null ? 0 : minute}
                  onOpen={() => onTab('terminals')}
                />
              )}
              {workersOf(workers, agent.agentId).map((worker) => (
                <WorkerRow key={worker.agentId} worker={worker} nested />
              ))}
            </Fragment>
          ))}
          {strays.map((worker) => (
            <WorkerRow key={worker.agentId} worker={worker} />
          ))}
        </Rows>
      ) : null}
      <Recent core={core} snapshot={snapshot} scope={scope} provider={provider} full={full} hostName={hostName} />
      {!open.length && !strays.length && !snapshot.recent.some((chat) => !scope || chat.project === scope.id) ? (
        <EmptyState icon="IconAgent" text={scope ? `No agents in ${scope.name}.` : 'No agents running.'} />
      ) : null}
    </>
  );
}

/* A host sends its whole view several times a second while an agent works; each row redraws only when its own
   item changed, which the hub keeps as the same object otherwise. */
const ChatRow = memo(function ChatRow({
  core,
  shown,
  place,
  minute,
}: {
  core: string;
  shown: AgentShown;
  place?: string;
  /** Counts down a trigger's time once a minute; 0 for a row without one, so the others keep still. */
  minute: number;
}) {
  return (
    <Row
      mark={<AgentFace agentId={shown.agentId} provider={shown.cli} group={shown.group} size={20} />}
      name={shown.name}
      title={shown.title}
      place={place}
      end={
        <>
          {shown.subagents ? <SubagentCount count={shown.subagents} /> : null}
          {shown.tasks ? <ShellCount count={shown.tasks} /> : null}
          <Trigger shown={shown} minute={minute} />
          <AgentState group={shown.group} />
        </>
      }
      onPress={() => router.push(`/device/${core}/chat/${shown.agentId}`)}
    />
  );
});

/** An agent running in a terminal, listed with the chats as the host's rail lists it. */
const TerminalAgentRow = memo(function TerminalAgentRow({
  shown,
  place,
  minute,
  onOpen,
}: {
  shown: AgentShown;
  place?: string;
  minute: number;
  onOpen: () => void;
}) {
  const { group } = shown;
  return (
    <Row
      mark={<AgentFace agentId={shown.agentId} provider={shown.cli} group={group} size={20} />}
      name={shown.name}
      title={shown.title}
      place={place}
      end={
        <>
          <Trigger shown={shown} minute={minute} />
          <AgentState group={group} />
        </>
      }
      onPress={onOpen}
    />
  );
});

/** How long until a trigger wakes the agent, while it waits rather than works. */
function Trigger({ shown, minute }: { shown: AgentShown; minute: number }) {
  if (shown.nextTrigger == null || shown.group === 'working' || shown.group === 'blocked') return null;
  return <TriggerTime at={Number(shown.nextTrigger)} now={minute * 60_000} />;
}

const makeStyles = () => {
  return StyleSheet.create({
    open: { marginTop: 10 },
  });
};
