import { Fragment, memo, useState, type ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';

import { formatAgo } from '@shared/lib/format';
import type { AgentShown, ProjectInfo, TeamLink } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { WorkerRow, workersOf } from '@/devices/RemoteWorker';
import { useMinute } from '@/ui/motion';
import { AgentFace } from '@/devices/Buddy';
import { AwayNote, AwaySheet, restore } from '@/devices/AwaySheet';
import { toggleFolded, useFolded } from '@/devices/folded';
import { useLive } from '@/devices/hub';
import { Icon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { EmptyState } from '@/ui/prompt';
import { AgentState, ShellCount, SubagentCount, TriggerTime, WakeTime } from '@/devices/AgentMarks';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { listed, Recent } from '@/devices/DeviceRecent';

export type Tab = 'agents' | 'terminals' | 'board';

/**
 * The open agents in the order the host lists them on every device: the main list, what wants the person first, then
 * the snoozed and then the settled ones.
 */
function openAgents(snapshot: Snapshot, scope: ProjectInfo | undefined, provider: string) {
  const agents = snapshot.inbox.agents.filter((agent) => listed(scope, provider, agent.project, agent.cli));
  return {
    main: agents.filter((agent) => agent.snoozedUntil == null && agent.settledAt == null),
    snoozed: agents.filter((agent) => agent.snoozedUntil != null && agent.settledAt == null),
    settled: agents.filter((agent) => agent.settledAt != null),
  };
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
  const { main: open, snoozed, settled } = openAgents(snapshot, scope, provider);
  const [held, setHeld] = useState<string>();
  const hold = (shown: AgentShown) => setHeld(shown.agentId);
  const onHold = full ? hold : undefined;
  const heldShown = held ? snapshot.inbox.agents.find((agent) => agent.agentId === held) : undefined;
  const awayRow = (agent: AgentShown) =>
    agent.session == null ? (
      <ChatRow key={agent.agentId} core={core} shown={agent} minute={minute} onHold={onHold} />
    ) : (
      <TerminalAgentRow key={agent.agentId} shown={agent} minute={minute} onOpen={() => onTab('terminals')} onHold={onHold} />
    );
  const workers = snapshot.inbox.remoteAgents;
  const leads = new Set(open.map((agent) => agent.agentId));
  const strays = scope
    ? []
    : workers.filter((worker) => !(worker.lead && leads.has(worker.lead)) && listed(undefined, provider, undefined, worker.cli));
  // A filtered list may have left a teammate's lead out, so it would hang from nothing.
  const teamOf = (agent: AgentShown) => (provider === 'all' ? agent.team : undefined);

  return (
    <>
      <AwayNote />
      {open.length || strays.length ? (
        <Rows style={styles.open}>
          {open.map((agent) => (
            <Fragment key={agent.agentId}>
              <TeamLines team={teamOf(agent)}>
                {agent.session == null ? (
                  <ChatRow core={core} shown={agent} minute={agent.nextTrigger == null ? 0 : minute} onHold={onHold} />
                ) : (
                  <TerminalAgentRow
                    shown={agent}
                    minute={agent.nextTrigger == null ? 0 : minute}
                    onOpen={() => onTab('terminals')}
                    onHold={onHold}
                  />
                )}
              </TeamLines>
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
      <AwayGroup label="Snoozed" fold="snoozed" agents={snoozed} row={awayRow} />
      <AwayGroup label="Settled" fold="settled" agents={settled} row={awayRow} />
      <Recent core={core} snapshot={snapshot} scope={scope} provider={provider} full={full} hostName={hostName} />
      <AwaySheet core={core} shown={heldShown} onClose={() => setHeld(undefined)} />
      {!open.length &&
      !strays.length &&
      !snoozed.length &&
      !settled.length &&
      !snapshot.recent.some((chat) => !scope || chat.project === scope.id) ? (
        <EmptyState icon="IconAgent" text={scope ? `No agents in ${scope.name}.` : 'No agents running.'} />
      ) : null}
    </>
  );
}

/** Down the middle of a lead's mark, which sits 8 into its row, and half a row, where a teammate's mark is centred. */
const SPINE = 18;
const MIDDLE = 24;

/** The spine and tick that hang a teammate's row under its lead, as the Mac's rail draws them. */
function TeamLines({ team, children }: { team?: TeamLink; children: ReactNode }) {
  const styles = useStyles(makeStyles);
  const member = team === 'member' || team === 'last';
  return (
    <View style={member ? styles.member : undefined}>
      {team === 'head' ? <View style={[styles.spine, { top: MIDDLE + 12, bottom: -2 }]} /> : null}
      {team === 'member' ? (
        <>
          <View style={[styles.spine, { top: -2, bottom: -2 }]} />
          <View style={styles.tick} />
        </>
      ) : null}
      {team === 'last' ? <View style={styles.corner} /> : null}
      {children}
    </View>
  );
}

/* A host sends its whole view several times a second while an agent works; each row redraws only when its own
   item changed, which the hub keeps as the same object otherwise. */
const ChatRow = memo(function ChatRow({
  core,
  shown,
  minute,
  onHold,
}: {
  core: string;
  shown: AgentShown;
  /** Counts down a trigger's time once a minute; 0 for a row without one, so the others keep still. */
  minute: number;
  /** Opens its snooze sheet; absent for a phone that can only watch. */
  onHold?: (shown: AgentShown) => void;
}) {
  const away = shown.snoozedUntil != null || shown.settledAt != null;
  return (
    <Row
      mark={
        <Faded away={away}>
          <AgentFace agentId={shown.agentId} provider={shown.cli} group={shown.group} size={20} />
        </Faded>
      }
      name={away ? undefined : shown.name}
      title={shown.title}
      detail={shown.settledAt != null ? formatAgo(Number(shown.settledAt), minute * 60_000) : undefined}
      dim={away}
      end={
        away ? (
          shown.settledAt == null && <WakeTime at={Number(shown.snoozedUntil)} now={minute * 60_000} />
        ) : (
          <>
            {shown.subagents ? <SubagentCount count={shown.subagents} /> : null}
            {shown.tasks ? <ShellCount count={shown.tasks} /> : null}
            <Trigger shown={shown} minute={minute} />
            <AgentState group={shown.group} />
          </>
        )
      }
      onPress={() => router.push(`/device/${core}/chat/${shown.agentId}`)}
      onLongPress={onHold && (() => onHold(shown))}
      after={shown.settledAt != null && onHold ? <RestoreButton core={core} shown={shown} /> : undefined}
    />
  );
});

/** A snoozed or settled agent's mark, faded as the Mac's rail fades it. */
function Faded({ away, children }: { away: boolean; children: ReactNode }) {
  return away ? <View style={{ opacity: 0.55 }}>{children}</View> : <>{children}</>;
}

/** Puts a settled agent back in the main list. */
function RestoreButton({ core, shown }: { core: string; shown: AgentShown }) {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const live = useLive(core);
  if (live.status !== 'open') return null;
  const connection = live.connection;
  return (
    <Pressable
      onPress={() => restore(connection, shown.agentId)}
      hitSlop={6}
      style={({ pressed }) => [styles.restore, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={`Restore ${shown.title}`}>
      <Text style={styles.restoreText}>Restore</Text>
    </Pressable>
  );
}

/** Snoozed or Settled under the main list, folding on a tap of its label as the Mac's rail does. */
function AwayGroup({
  label,
  fold,
  agents,
  row,
}: {
  label: string;
  fold: string;
  agents: AgentShown[];
  row: (agent: AgentShown) => ReactNode;
}) {
  const colors = useColors();
  const folded = useFolded(fold);
  if (!agents.length) return null;
  return (
    <>
      <Pressable
        onPress={() => toggleFolded(fold)}
        accessibilityRole="button"
        accessibilityState={{ expanded: !folded }}
        accessibilityLabel={`${label}, ${agents.length}`}>
        <SectionLabel
          count={agents.length}
          action={
            <View style={{ transform: [{ rotate: folded ? '0deg' : '90deg' }] }}>
              <Icon name="IconChevron" size={12} color={colors.tertiary} />
            </View>
          }>
          {label}
        </SectionLabel>
      </Pressable>
      {folded ? null : <Rows>{agents.map(row)}</Rows>}
    </>
  );
}

/** An agent running in a terminal, listed with the chats as the host's rail lists it. */
const TerminalAgentRow = memo(function TerminalAgentRow({
  shown,
  minute,
  onOpen,
  onHold,
}: {
  shown: AgentShown;
  minute: number;
  onOpen: () => void;
  onHold?: (shown: AgentShown) => void;
}) {
  const { group } = shown;
  const snoozed = shown.snoozedUntil != null && shown.settledAt == null;
  const away = snoozed || shown.settledAt != null;
  return (
    <Row
      mark={
        <Faded away={away}>
          <AgentFace agentId={shown.agentId} provider={shown.cli} group={group} size={20} />
        </Faded>
      }
      name={away ? undefined : shown.name}
      title={shown.title}
      dim={away}
      end={
        snoozed ? (
          <WakeTime at={Number(shown.snoozedUntil)} now={minute * 60_000} />
        ) : away ? null : (
          <>
            <Trigger shown={shown} minute={minute} />
            <AgentState group={group} />
          </>
        )
      }
      onPress={onOpen}
      onLongPress={onHold && (() => onHold(shown))}
    />
  );
});

/** How long until a trigger wakes the agent, while it waits rather than works. */
function Trigger({ shown, minute }: { shown: AgentShown; minute: number }) {
  if (shown.nextTrigger == null || shown.group === 'working' || shown.group === 'blocked') return null;
  return <TriggerTime at={Number(shown.nextTrigger)} now={minute * 60_000} />;
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    open: { marginTop: 10 },
    member: { paddingLeft: 22 },
    spine: { position: 'absolute', left: SPINE, width: 1, backgroundColor: colors.treeSpine },
    tick: { position: 'absolute', top: MIDDLE, left: SPINE, width: 8, height: 1, backgroundColor: colors.treeSpine },
    corner: {
      position: 'absolute',
      top: -2,
      left: SPINE,
      width: 8,
      height: MIDDLE + 3,
      borderLeftWidth: 1,
      borderBottomWidth: 1,
      borderColor: colors.treeSpine,
      borderBottomLeftRadius: 4,
    },
    restore: {
      height: 28,
      marginLeft: 6,
      paddingHorizontal: 10,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
      justifyContent: 'center',
    },
    restoreText: { fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.ink },
  });
};
