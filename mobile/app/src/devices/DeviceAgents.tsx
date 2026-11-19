import { Fragment, memo, useState, type ReactNode } from 'react';
import { LayoutAnimation, Pressable, StyleSheet, View } from 'react-native';
import { router } from 'expo-router';

import type { AgentShown, ProjectInfo, RemoteAgent } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { WorkerRow } from '@/devices/RemoteWorker';
import { TreeFold, TreeLines, teamFold } from '@/devices/AgentTree';
import { NeedsYouMark } from '@/devices/NeedsYou';
import { above, topOf, tree, type Placed } from '@/devices/tree';
import { useAnimating, useMinute } from '@/ui/motion';
import { AgentFace } from '@/devices/Buddy';
import { AwayNote, AwaySheet } from '@/devices/AwaySheet';
import { toggleFolded, useFolded, useFoldedKeys } from '@/devices/folded';
import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { EmptyState } from '@/ui/prompt';
import { LinePlace, ShellCount, SubagentCount, TurnMark, WakeTime } from '@/devices/AgentMarks';
import { ChiefRow } from '@/calls/ChiefRow';
import { useColors, useStyles } from '@/ui/theme';
import { listed, Recent } from '@/devices/DeviceRecent';

export type Tab = 'agents' | 'terminals' | 'calls';

/** An agent the list draws: one here, or a worker an agent here started on a linked host. */
type Item = {
  id: string;
  lead?: string;
  project?: string;
  snoozed: boolean;
  away: boolean;
  chiefLed: boolean;
  agent?: AgentShown;
  worker?: RemoteAgent;
};

const leafOf = (item: Item) => item;

/**
 * The agents the list draws, each hung under its lead: every open agent and worker shown, a chief only where a list
 * filtered to one kind has no @Chief row, and a settled one only while its team still works. A filtered list is flat,
 * as it may have left a lead out; an agent a chief started hangs under that chief's row.
 */
function itemsOf(snapshot: Snapshot, scope: ProjectInfo | undefined, provider: string) {
  const flat = provider !== 'all';
  const chiefs = new Set(snapshot.inbox.agents.filter((agent) => agent.chief).map((agent) => agent.agentId));
  const leadOf = (lead: string | undefined) => (flat || !lead || chiefs.has(lead) ? undefined : lead);
  const item = (shown: AgentShown | RemoteAgent, worker?: RemoteAgent): Item => ({
    id: shown.agentId,
    lead: leadOf(shown.lead),
    chiefLed: !flat && shown.lead != null && chiefs.has(shown.lead),
    project: shown.project,
    snoozed: shown.snoozedUntil != null,
    away: shown.snoozedUntil != null || shown.settledAt != null,
    agent: worker ? undefined : shown,
    worker,
  });
  const here = snapshot.inbox.agents
    .filter((agent) => listed(scope, provider, agent.project, agent.cli) && (flat || !agent.chief))
    .map((agent) => item(agent));
  const seen = new Set(here.map((agent) => agent.id));
  const workers = snapshot.inbox.remoteAgents
    .filter((worker) => !seen.has(worker.agentId) && listed(undefined, provider, undefined, worker.cli))
    .map((worker) => item(worker, worker));
  let items = [...here, ...workers];
  if (scope) {
    const top = topOf(items);
    items = items.filter((item) => !item.worker || item.project === scope.id || seen.has(top.get(item.id) ?? ''));
  }
  const leading = above(
    items,
    items.filter((item) => !item.away).map((item) => item.id),
  );
  return {
    main: items.filter((item) => !item.away || leading.has(item.id)),
    snoozed: items.filter((item) => item.snoozed && !leading.has(item.id)),
  };
}

/** One project's chief and the agents under it; agents of no project shown go last with no chief. */
type Team = { id: string; project?: ProjectInfo; items: Item[] };

/**
 * A project's team as the Mac's rail splits it: the trees whose top agent its chief started, which hang under the
 * chief's row, and the person's own, which stand at the root below them.
 */
function byStarter(items: Item[]) {
  const byId = new Map(items.map((item) => [item.id, item]));
  const top = topOf(items);
  const led = (item: Item) => byId.get(top.get(item.id) ?? '')?.chiefLed === true;
  return { led: items.filter(led), own: items.filter((item) => !led(item)) };
}

/** Where a project goes in the list by how the core says it stands, as the Mac and the notch order it. */
const RANK: Record<string, number> = { needsYou: 0, atWork: 1 };
const rankOf = (project: ProjectInfo) => RANK[project.standing ?? 'atRest'] ?? 2;

/**
 * The list as teams. In one project, its chief leads it. With every project shown, each project with a chief or agents
 * is a team, ordered by how the core says each stands, ties in the host's order of projects; an agent goes with the
 * project of the agent at the top of its tree. A list filtered to one kind of agent has no chiefs.
 */
function teamsOf(snapshot: Snapshot, open: Item[], scope: ProjectInfo | undefined, provider: string): Team[] {
  if (provider !== 'all') return [{ id: '', items: open }];
  if (scope) return [{ id: scope.id, project: scope, items: open }];
  const shown = snapshot.workspace.projects.filter(
    (project) => project.chief != null || project.chiefHome != null || (project.agents ?? 0) > 0,
  );
  const ranked = shown
    .map((project, index) => ({ project, index, rank: rankOf(project) }))
    .sort((a, b) => a.rank - b.rank || a.index - b.index)
    .map(({ project }) => project);
  const byId = new Map(open.map((item) => [item.id, item]));
  const top = topOf(open);
  const home = (item: Item) => byId.get(top.get(item.id) ?? '')?.project;
  const teams: Team[] = ranked.map((project) => ({
    id: project.id,
    project,
    items: open.filter((item) => home(item) === project.id),
  }));
  const placed = new Set(shown.map((project) => project.id));
  const loose = open.filter((item) => {
    const project = home(item);
    return !project || !placed.has(project);
  });
  if (loose.length) teams.push({ id: '', items: loose });
  return teams;
}

/** The teams in the order they had when a finger came down, while it stays; new ones after them. */
function keptStill(teams: Team[], held: string[] | undefined): Team[] {
  if (!held) return teams;
  const at = new Map(held.map((id, index) => [id, index]));
  return teams
    .map((team, index) => ({ team, rank: at.get(team.id) ?? held.length + index }))
    .sort((a, b) => a.rank - b.rank)
    .map(({ team }) => team);
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
  const t = useT();
  const minute = useMinute();
  const { main: open, snoozed } = itemsOf(snapshot, scope, provider);
  const foldedKeys = useFoldedKeys();
  const folded = new Set(foldedKeys.flatMap((key) => (key.startsWith(teamFold('')) ? [key.slice(teamFold('').length)] : [])));
  const animating = useAnimating();
  // The projects' order while a finger is on the list, so a tap never lands on a row that moved under it.
  const [touchOrder, setTouchOrder] = useState<string[]>();
  const teams = keptStill(teamsOf(snapshot, open, scope, provider), touchOrder);
  const order = teams.map((team) => team.id).join('|');
  const [shownOrder, setShownOrder] = useState(order);
  if (shownOrder !== order) {
    setShownOrder(order);
    // A project that moves glides to its place, as the list redraws in the new order.
    if (animating) LayoutAnimation.configureNext(LayoutAnimation.create(280, 'easeInEaseOut', 'opacity'));
  }
  const [snoozing, setSnoozing] = useState<string>();
  const hold = (shown: AgentShown) => setSnoozing(shown.agentId);
  const onHold = full ? hold : undefined;
  const snoozingShown = snoozing ? snapshot.inbox.agents.find((agent) => agent.agentId === snoozing) : undefined;
  const nameOf = (id: string | undefined) =>
    id === undefined
      ? undefined
      : ((snapshot.inbox.agents.find((agent) => agent.agentId === id) ?? snapshot.inbox.remoteAgents.find((agent) => agent.agentId === id))
          ?.name ?? undefined);
  const row = (placed: Placed<Item>, folds: boolean) => {
    const { item } = placed;
    const fold = folds && placed.below > 0 ? { count: placed.below, open: !folded.has(item.id) } : undefined;
    const lead = nameOf(placed.leadApart);
    return (
      <NeedsYouMark key={item.id} id={item.id} on={item.agent?.group === 'blocked'}>
        <TreeLines depth={placed.depth} lines={placed.lines} open={placed.open}>
          {item.worker ? (
            <WorkerRow worker={item.worker} lead={lead} foldCount={fold?.count} foldOpen={fold?.open} />
          ) : !item.agent ? null : item.agent.session == null ? (
            <ChatRow
              core={core}
              shown={item.agent}
              minute={minute}
              onHold={onHold}
              lead={lead}
              foldCount={fold?.count}
              foldOpen={fold?.open}
            />
          ) : (
            <TerminalAgentRow
              shown={item.agent}
              minute={minute}
              onOpen={() => onTab('terminals')}
              onHold={onHold}
              lead={lead}
              foldCount={fold?.count}
              foldOpen={fold?.open}
            />
          )}
        </TreeLines>
      </NeedsYouMark>
    );
  };
  const away = tree(snoozed, leafOf, 0, new Set());

  return (
    <>
      <AwayNote />
      {teams.some((team) => team.project || team.items.length) ? (
        <View
          onTouchStart={() => setTouchOrder((now) => now ?? teams.map((team) => team.id))}
          onTouchEnd={() => setTouchOrder(undefined)}
          onTouchCancel={() => setTouchOrder(undefined)}>
          <Rows style={styles.open}>
            {teams.map((team) => {
              const { led, own } = team.project ? byStarter(team.items) : { led: [], own: team.items };
              const placed = tree(led, leafOf, 1, folded);
              return (
                <Fragment key={team.id}>
                  {team.project ? (
                    <ChiefTeamRow
                      core={core}
                      project={team.project}
                      snapshot={snapshot}
                      mixed={!scope}
                      minute={minute}
                      open={placed.length > 0}
                    />
                  ) : null}
                  {placed.map((placed) => row(placed, true))}
                  {tree(own, leafOf, 0, folded).map((placed) => row(placed, true))}
                </Fragment>
              );
            })}
          </Rows>
        </View>
      ) : null}
      <AwayGroup label={t('Snoozed')} fold="snoozed" count={away.length}>
        {away.map((placed) => row(placed, false))}
      </AwayGroup>
      <Recent core={core} snapshot={snapshot} scope={scope} provider={provider} full={full} hostName={hostName} />
      <AwaySheet core={core} shown={snoozingShown} onClose={() => setSnoozing(undefined)} />
      {!open.length && !snoozed.length && !snapshot.recent.some((chat) => !scope || chat.project === scope.id) ? (
        <EmptyState icon="IconAgent" text={scope ? t('No agents in {project}.', { project: scope.name }) : t('No agents running.')} />
      ) : null}
    </>
  );
}

/** A project's @Chief row, marked for the list's edge while it needs the person. */
function ChiefTeamRow({
  core,
  project,
  snapshot,
  mixed,
  minute,
  open,
}: {
  core: string;
  project: ProjectInfo;
  snapshot: Snapshot;
  mixed: boolean;
  minute: number;
  open: boolean;
}) {
  const shown = snapshot.inbox.agents.find((agent) => agent.chief && agent.project === project.id);
  return (
    <NeedsYouMark id={`chief:${project.id}`} on={shown?.group === 'blocked'}>
      <TreeLines open={open}>
        <ChiefRow core={core} project={project} shown={shown} mixed={mixed} now={minute * 60_000} />
      </TreeLines>
    </NeedsYouMark>
  );
}

/** What a row of the tree carries besides its agent: the lead it names, and the fold of the team under it. */
type Hung = { lead?: string; foldCount?: number; foldOpen?: boolean };

/* A host sends its whole view several times a second while an agent works; each row redraws only when its own
   item changed, which the hub keeps as the same object otherwise. */
const ChatRow = memo(function ChatRow({
  core,
  shown,
  minute,
  onHold,
  lead,
  foldCount,
  foldOpen = true,
}: Hung & {
  core: string;
  shown: AgentShown;
  /** Counts down a trigger's time once a minute; 0 for a row without one, so the others keep still. */
  minute: number;
  /** Opens its snooze sheet; absent for a phone that can only watch. */
  onHold?: (shown: AgentShown) => void;
}) {
  const away = shown.snoozedUntil != null;
  const settled = shown.settledAt != null;
  return (
    <Row
      mark={
        <Faded away={away || settled}>
          <AgentFace agentId={shown.agentId} provider={shown.cli} turn={shown.turn} size={20} />
        </Faded>
      }
      name={away ? undefined : shown.name}
      lead={lead}
      title={shown.title}
      dim={away || settled}
      after={foldCount ? <TreeFold agentId={shown.agentId} count={foldCount} open={foldOpen} /> : undefined}
      end={
        away ? (
          <WakeTime at={Number(shown.snoozedUntil)} now={minute * 60_000} />
        ) : (
          <>
            {shown.subagents ? <SubagentCount count={shown.subagents} /> : null}
            {shown.tasks ? <ShellCount count={shown.tasks} /> : null}
            {shown.line ? <LinePlace line={shown.line} /> : null}
            <TurnMark turn={shown.turn} now={minute * 60_000} />
          </>
        )
      }
      onPress={() => router.push(`/device/${core}/chat/${shown.agentId}`)}
      onLongPress={onHold && (() => onHold(shown))}
    />
  );
});

/** A snoozed agent's mark, faded as the Mac's rail fades it. */
function Faded({ away, children }: { away: boolean; children: ReactNode }) {
  return away ? <View style={{ opacity: 0.55 }}>{children}</View> : <>{children}</>;
}

/** Snoozed under the main list, folding on a tap of its label as the Mac's rail does. */
function AwayGroup({ label, fold, count, children }: { label: string; fold: string; count: number; children: ReactNode }) {
  const colors = useColors();
  const folded = useFolded(fold);
  if (!count) return null;
  return (
    <>
      <Pressable
        onPress={() => toggleFolded(fold)}
        accessibilityRole="button"
        accessibilityState={{ expanded: !folded }}
        accessibilityLabel={`${label}, ${count}`}>
        <SectionLabel
          count={count}
          action={
            <View style={{ transform: [{ rotate: folded ? '0deg' : '90deg' }] }}>
              <Icon name="IconChevron" size={12} color={colors.tertiary} />
            </View>
          }>
          {label}
        </SectionLabel>
      </Pressable>
      {folded ? null : <Rows>{children}</Rows>}
    </>
  );
}

/** An agent running in a terminal, listed with the chats as the host's rail lists it. */
const TerminalAgentRow = memo(function TerminalAgentRow({
  shown,
  minute,
  onOpen,
  onHold,
  lead,
  foldCount,
  foldOpen = true,
}: Hung & {
  shown: AgentShown;
  minute: number;
  onOpen: () => void;
  onHold?: (shown: AgentShown) => void;
}) {
  const away = shown.snoozedUntil != null;
  const settled = shown.settledAt != null;
  return (
    <Row
      mark={
        <Faded away={away || settled}>
          <AgentFace agentId={shown.agentId} provider={shown.cli} turn={shown.turn} size={20} />
        </Faded>
      }
      name={away ? undefined : shown.name}
      lead={lead}
      title={shown.title}
      dim={away || settled}
      after={foldCount ? <TreeFold agentId={shown.agentId} count={foldCount} open={foldOpen} /> : undefined}
      end={away ? <WakeTime at={Number(shown.snoozedUntil)} now={minute * 60_000} /> : <TurnMark turn={shown.turn} now={minute * 60_000} />}
      onPress={onOpen}
      onLongPress={onHold && (() => onHold(shown))}
    />
  );
});

const makeStyles = () => StyleSheet.create({ open: { marginTop: 10 } });
