import { useState } from 'react';
import { Text, View, StyleSheet } from 'react-native';
import { router } from 'expo-router';
import type { ConnectionLike } from '@sikemux/native';

import {
  cardQuestion,
  COLUMN_LABELS,
  RUN_WAITS_ON_PERSON,
  shortTitle,
  type BoardCard,
  type BoardColumn,
  type ProjectBoard,
} from '@shared/agents/agentBoard';
import { kindLabel, outcomeLabel, rowKeys, whoLabel, type AgentMessageRows } from '@shared/agents/agentMessages';
import { formatAge } from '@shared/lib/format';
import type { AgentQuestion, ProjectInfo, Snapshot } from '@/core/protocol';
import { useHostCall } from '@/core/hostCalls';
import { projectAt, projectName } from '@/devices/words';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { EmptyState } from '@/ui/prompt';
import { Dot, NeedsYou, Working } from '@/ui/status';
import { fonts, type Palette, typeFor, useColors, useStyles } from '@/ui/theme';

const COLUMNS: readonly BoardColumn[] = ['needs', 'progress', 'open', 'done'];

const NO_QUESTIONS: AgentQuestion[] = [];

/** Something on the host changed that may have moved cards: an agent started, finished or asked. */
function seenOf(snapshot: Snapshot): string {
  const chats = snapshot.chats.map((chat) => `${chat.agentId}:${chat.group}:${chat.running}:${chat.activeAt}`);
  const terminals = snapshot.sessions.map((session) => `${session.agentId}:${session.agentState}`);
  return [...chats, ...terminals].join(' ');
}

/** A project's board, read only: cards move on the Mac or by the agents themselves. Unscoped, a row per board. */
export function Board({
  core,
  connection,
  snapshot,
  scope,
  onScope,
}: {
  core: string;
  connection: ConnectionLike | undefined;
  snapshot: Snapshot;
  scope?: ProjectInfo;
  onScope: (project: string) => void;
}) {
  const seen = seenOf(snapshot);
  const read = useHostCall<AgentQuestion[]>(connection, 'questions', null, seen);
  const questions = read.data ?? NO_QUESTIONS;
  return (
    <>
      {read.problem ? <Problem what="the agents' questions" text={read.problem} /> : null}
      {scope ? (
        <OneBoard core={core} connection={connection} snapshot={snapshot} scope={scope} questions={questions} seen={seen} />
      ) : (
        <AllBoards connection={connection} snapshot={snapshot} seen={seen} onScope={onScope} />
      )}
    </>
  );
}

function Problem({ what, text }: { what: string; text: string }) {
  return <EmptyState icon="IconWarning" text={`Couldn't read ${what}: ${text}`} />;
}

function OneBoard({
  core,
  connection,
  snapshot,
  scope,
  questions,
  seen,
}: {
  core: string;
  connection: ConnectionLike | undefined;
  snapshot: Snapshot;
  scope: ProjectInfo;
  questions: readonly AgentQuestion[];
  seen: string;
}) {
  const read = useHostCall<ProjectBoard>(connection, 'board', { project: scope.path }, seen);
  if (read.problem) return <Problem what="the board" text={read.problem} />;
  if (!read.data) return null;
  const cards = [...read.data.cards].sort((a, b) => b.updatedAt - a.updatedAt);
  const byId = new Map(cards.map((card) => [card.id, card]));
  return (
    <>
      {cards.length ? null : <EmptyState icon="IconBoard" text={`No cards on ${scope.name}'s board.`} />}
      {COLUMNS.map((column) => {
        const shown = cards.filter((card) => card.column === column);
        if (!shown.length) return null;
        return (
          <View key={column}>
            <SectionLabel count={shown.length}>{COLUMN_LABELS[column]}</SectionLabel>
            <Rows>
              {shown.map((card) => (
                <CardRow key={card.id} core={core} card={card} cards={cards} byId={byId} snapshot={snapshot} questions={questions} />
              ))}
            </Rows>
          </View>
        );
      })}
      <BetweenAgents connection={connection} snapshot={snapshot} project={scope.path} />
    </>
  );
}

const MESSAGES_SHOWN = 20;

/** Who messaged whom among the project's agents, read again whenever the host logs another. */
function BetweenAgents({ connection, snapshot, project }: { connection: ConnectionLike | undefined; snapshot: Snapshot; project: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [logged, setLogged] = useState(project);
  const latest = snapshot.inbox.agentMessages[logged] ?? 0;
  const read = useHostCall<AgentMessageRows>(connection, 'agentMessageRows', { project }, String(latest));
  const answered = read.data?.project;
  if (answered && answered !== logged) setLogged(answered);
  if (read.problem) return <Problem what="the messages between agents" text={read.problem} />;
  if (!read.data) return null;
  const cliOf = (id: string) =>
    snapshot.chats.find((known) => known.agentId === id)?.provider ??
    snapshot.sessions.find((session) => session.agentId === id)?.agentType;
  const rows = read.data.rows.slice(0, MESSAGES_SHOWN);
  if (!rows.length) return null;
  const keys = rowKeys(rows);
  return (
    <View>
      <SectionLabel>Between agents</SectionLabel>
      <Rows>
        {rows.map((row, index) => {
          const cli = row.from ? cliOf(row.from) : undefined;
          const tone = row.tone;
          return (
            <Row
              key={keys[index]}
              mark={cli ? <AgentIcon provider={cli} size={20} /> : <Dot color={colors.rest} size={9} hollow />}
              title={`${whoLabel(row.from, row.fromName)} to ${whoLabel(row.to, row.toName)}`}
              detail={
                <>
                  {kindLabel(row.kind)} ·{' '}
                  <Text style={tone === 'held' ? styles.held : tone === 'failed' ? styles.failed : undefined}>{outcomeLabel(row)}</Text>
                </>
              }
              end={<Text style={styles.ago}>{formatAge(row.sentAt)}</Text>}
            />
          );
        })}
      </Rows>
    </View>
  );
}

function CardRow({
  core,
  card,
  cards,
  byId,
  snapshot,
  questions,
}: {
  core: string;
  card: BoardCard;
  cards: readonly BoardCard[];
  byId: ReadonlyMap<number, BoardCard>;
  snapshot: Snapshot;
  questions: readonly AgentQuestion[];
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const column = card.column;
  const chat = card.owner ? snapshot.chats.find((known) => known.agentId === card.owner) : undefined;
  const terminal = card.owner ? snapshot.sessions.find((session) => session.agentId === card.owner && session.running) : undefined;
  const owner = chat?.name ?? terminal?.agentName ?? card.ownerName;
  const cli = chat?.provider ?? terminal?.agentType ?? card.ownerCli;
  const group = snapshot.inbox.agents.find((agent) => agent.agentId === card.owner)?.group;
  const question = column === 'needs' ? cardQuestion(card, questions) : undefined;
  const handedBack = card.auto && RUN_WAITS_ON_PERSON.has(card.auto.stage) && card.status !== 'done' ? card.auto.note : null;
  const reviewing = card.auto?.stage === 'reviewing' ? card.auto.note : null;
  const acceptance = card.acceptance && card.status !== 'done' ? `Done when: ${card.acceptance}` : null;
  const under = cards.filter((other) => other.parent === card.id);
  const done = under.filter((other) => other.status === 'done').length;
  const waits = card.waitsOn.map((id) => byId.get(id)).filter((other): other is BoardCard => !!other);
  const after = waits[0] ?? (card.dependsOn.length ? byId.get(card.dependsOn[0]) : undefined);
  const filed = card.ticket ? null : card.ticketAbove;
  const parent = card.parent === null ? undefined : byId.get(card.parent);
  const place = card.ticket ? (
    <>
      <Text style={styles.key}>{card.ticket.key}</Text>
      {card.ticket.source ? ` ${card.ticket.source}` : ''}
    </>
  ) : filed ? (
    <>
      under <Text style={styles.key}>{filed}</Text>
    </>
  ) : parent ? (
    `under ${shortTitle(parent.title)}`
  ) : null;
  const parts = [
    owner ? <Text style={styles.owner}>{owner}</Text> : 'Nobody yet',
    place,
    under.length ? <Text style={styles.key}>{`${done}/${under.length}`}</Text> : null,
    after ? `after ${shortTitle(after.title)}` : null,
  ].filter((part) => part !== null);
  const state =
    card.status === 'done' ? (
      <Icon name="IconCheck" size={13} color={colors.tertiary} />
    ) : column === 'needs' || group === 'blocked' ? (
      <NeedsYou />
    ) : group === 'working' ? (
      <Working />
    ) : group === 'done' ? (
      <Dot color={colors.accent} />
    ) : null;
  return (
    <Row
      mark={cli ? <AgentIcon provider={cli} size={20} /> : <Dot color={colors.rest} size={9} hollow />}
      title={card.title}
      dim={card.status === 'done'}
      detail={
        question ? (
          <>
            <Text style={styles.owner}>{question.name}</Text> asks: {question.text}
          </>
        ) : (
          (handedBack ?? reviewing ?? acceptance ?? undefined)
        )
      }
      detailLines={2}
      under={parts.map((part, index) => (
        <Text key={index}>
          {index > 0 ? ' · ' : ''}
          {part}
        </Text>
      ))}
      end={state}
      onPress={chat ? () => router.push(`/device/${core}/chat/${chat.agentId}`) : undefined}
    />
  );
}

function AllBoards({
  connection,
  snapshot,
  seen,
  onScope,
}: {
  connection: ConnectionLike | undefined;
  snapshot: Snapshot;
  seen: string;
  onScope: (project: string) => void;
}) {
  const colors = useColors();
  const read = useHostCall<ProjectBoard[]>(connection, 'boards', null, seen);
  if (read.problem) return <Problem what="the boards" text={read.problem} />;
  if (!read.data) return null;
  const boards = read.data.filter((board) => board.cards.length > 0);
  if (!boards.length) return <EmptyState icon="IconBoard" text="No board has cards yet." />;
  return (
    <>
      <SectionLabel>Boards</SectionLabel>
      <Rows>
        {boards.map((board) => {
          const project = projectAt(snapshot, board.project);
          const counts = COLUMNS.map((column) => {
            const count = board.cards.filter((card) => card.column === column).length;
            return count ? `${count} ${COLUMN_LABELS[column].toLowerCase()}` : null;
          }).filter(Boolean);
          const needs = board.cards.some((card) => card.column === 'needs');
          return (
            <Row
              key={board.project}
              mark={<Icon name="IconFolder" size={18} color={colors.live} />}
              title={projectName(snapshot, board.project)}
              detail={counts.join(' · ')}
              end={needs ? <NeedsYou /> : null}
              onPress={project ? () => onScope(project.id) : undefined}
            />
          );
        })}
      </Rows>
    </>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    owner: { fontFamily: fonts.uiMedium, color: colors.secondary },
    key: { fontFamily: fonts.mono, fontSize: 11 },
    held: { color: colors.warn },
    failed: { color: colors.danger },
    ago: { ...type.meta, fontVariant: ['tabular-nums'] },
  });
};
