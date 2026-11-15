import { useContext, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';

import { agentLabel } from '@shared/agents/catalog';
import { said, stepLabel } from '@shared/calls/words';
import type { ChatMessage, ResultRow, TurnStep } from '@shared/wire.generated';
import type { ConnectionLike } from '@sikemux/native';
import { callHost } from '@/core/hostCalls';
import { HostContext } from '@/chat/hostFiles';
import { ActsContext } from '@/chat/transcriptContext';
import { Dot } from '@/devices/AgentMarks';
import { namedAgent, OwnMark, useCrew } from '@/devices/Crew';
import { useT } from '@/i18n';
import { Chip } from '@/ui/controls';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/**
 * A worker's finished work, as the Mac's lead chat shows it: its mark and name, what it worked on, where its work is and
 * how its checks stand, Try it and Undo, and a chevron that opens the worker's own chat.
 */
export function ResultCard({ row, connection }: { row: ResultRow; connection?: ConnectionLike }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const core = useContext(HostContext);
  const acts = useContext(ActsContext) && !!connection;
  const named = namedAgent(useCrew(), row.agentId);
  const [taking, setTaking] = useState<TurnStep>();
  const facts = said({ state: 'done', since: row.at, done: row.done }, agentLabel(named?.provider ?? 'agent'), row.at, t)?.parts.join(
    ' · ',
  );
  const tries = row.done.next?.some((next) => next.step === 'tryIt');
  const handle = row.name ? `@${row.name}` : undefined;

  const take = async (step: TurnStep) => {
    if (taking || !connection) return;
    setTaking(step);
    try {
      await callHost(connection, 'takeStep', { agentId: row.agentId, step });
      haptics.success();
    } catch {
      haptics.failure();
    } finally {
      setTaking(undefined);
    }
  };
  const chip = (step: TurnStep, primary: boolean) => {
    const label = stepLabel(step, t);
    return label ? <Chip title={label} primary={primary} busy={taking === step} onPress={() => void take(step)} /> : null;
  };

  return (
    <Pressable
      onPress={
        core
          ? () => {
              haptics.select();
              router.push(`/device/${core}/chat/${row.agentId}`);
            }
          : undefined
      }
      style={({ pressed }) => [styles.card, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={[handle, row.title, facts].filter(Boolean).join(', ')}>
      <OwnMark agentId={row.agentId} provider={named?.provider ?? 'agent'} chief={named?.chief} size={17} still />
      <Text style={styles.name}>{handle ?? row.title}</Text>
      {handle ? (
        <Text style={styles.title} numberOfLines={1}>
          {row.title}
        </Text>
      ) : null}
      {facts ? (
        <View style={styles.facts}>
          <Dot color={colors.accent} size={7} />
          <Text style={styles.factsText} numberOfLines={1}>
            {facts}
          </Text>
        </View>
      ) : null}
      <View style={styles.spacer} />
      {acts && tries ? chip('tryIt', true) : null}
      {acts && row.undo ? chip('undo', false) : null}
      <Icon name="IconChevron" size={11} color={colors.inkDim} />
    </Pressable>
  );
}

/** When a message happened: when its turn ended, or when the person sent it. */
const timeOf = (message: ChatMessage) => message.endedAt ?? message.sentAt;

/** The workers' finished work that goes after each row, by its index: what finished after it and before the next timed row. */
export function placeResults(results: readonly ResultRow[] | undefined, rows: readonly ChatMessage[]): Map<number, ResultRow[]> {
  const placed = new Map<number, ResultRow[]>();
  if (!results?.length) return placed;
  let next: number | undefined;
  for (let index = rows.length - 1; index >= 0; index -= 1) {
    const at = timeOf(rows[index]);
    if (at == null) continue;
    const found = results.filter((result) => result.at >= at && (next == null || result.at < next));
    if (found.length) placed.set(index, found);
    next = at;
  }
  return placed;
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    card: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      minHeight: 36,
      paddingLeft: 12,
      paddingRight: 10,
      marginTop: 8,
      marginBottom: 4,
      borderRadius: 9,
      backgroundColor: colors.raised,
    },
    name: { flexShrink: 0, fontFamily: fonts.uiSemibold, fontSize: 12.5, color: colors.ink },
    title: { flexShrink: 1, minWidth: 0, fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    facts: { flexDirection: 'row', alignItems: 'center', gap: 5, flexShrink: 0 },
    factsText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    spacer: { flex: 1 },
  });
