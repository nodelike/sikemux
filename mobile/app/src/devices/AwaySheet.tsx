import { useState } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import type { ConnectionLike } from '@sikemux/native';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';
import type { AgentShown } from '@shared/wire.generated';
import { MenuItem } from '@/chat/ChatMenu';
import { callHost, useHostCall } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { useLive } from '@/devices/hub';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** What was just snoozed or settled from this phone, with how to take it back. */
type Note = { text: string; undo: () => void };

const NOTE_MS = 6000;
const note = externalValue<Note | undefined>(undefined);
let clearing: ReturnType<typeof setTimeout> | undefined;

function tell(next: Note) {
  clearTimeout(clearing);
  note.set(next);
  clearing = setTimeout(() => note.set(undefined), NOTE_MS);
}

/** The time `at` on this phone's clock, with its weekday when it is not today. */
function clock(at: number): string {
  const when = new Date(at);
  const time = when.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });
  if (when.toDateString() === new Date().toDateString()) return time;
  return `${when.toLocaleDateString(undefined, { weekday: 'short' })} ${time}`;
}

function snooze(connection: ConnectionLike, agentIds: string[], until: number | null, typed?: string) {
  return callHost(connection, 'snoozeAgents', { agentIds, until, ...(typed ? { typed } : {}) });
}

function settle(connection: ConnectionLike, agentIds: string[], settled: boolean) {
  return callHost(connection, 'settleAgents', { agentIds, settled });
}

/** Restores a settled agent to the main list, as its row's Restore does. */
export function restore(connection: ConnectionLike, agentId: string) {
  haptics.tap();
  void settle(connection, [agentId], false).catch(() => haptics.failure());
}

/**
 * Snoozing or settling one agent: the host's own times, a time typed in words that the host reads, and Settle; Wake
 * now in place of the times once it is snoozed, and Restore in place of Settle once it is settled.
 */
export function AwaySheet({
  core,
  shown,
  onClose,
  onDone,
}: {
  core: string;
  /** The agent the sheet is about; none while it is shut. */
  shown?: AgentShown;
  onClose: () => void;
  /** Runs once something was snoozed, settled or brought back. */
  onDone?: () => void;
}) {
  return (
    <Sheet visible={!!shown} onClose={onClose}>
      {shown ? <Choices key={shown.agentId} core={core} shown={shown} onClose={onClose} onDone={onDone} /> : null}
    </Sheet>
  );
}

function Choices({ core, shown, onClose, onDone }: { core: string; shown: AgentShown; onClose: () => void; onDone?: () => void }) {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const t = useT();
  const live = useLive(core);
  const connection = live.status === 'open' ? live.connection : undefined;
  const presets = useHostCall(connection, 'snoozePresets', {}, shown.agentId);
  const [typed, setTyped] = useState('');
  const [failed, setFailed] = useState<string>();
  const ids = [shown.agentId];
  const snoozed = shown.snoozedUntil != null;
  const settled = shown.settledAt != null;

  const act = (work: (connection: ConnectionLike) => Promise<unknown>, said?: Note) => {
    if (!connection) {
      haptics.failure();
      setFailed(t('Not connected to the host.'));
      return;
    }
    haptics.tap();
    work(connection).then(
      () => {
        if (said) tell(said);
        onClose();
        onDone?.();
      },
      (error: unknown) => {
        haptics.failure();
        setFailed(problem(error));
      },
    );
  };
  const snoozeUntil = (until: number | null, words?: string) =>
    act(
      (connection) => snooze(connection, ids, until, words),
      until || words
        ? {
            text: until ? t('Snoozed until {time}', { time: clock(until) }) : t('Snoozed'),
            undo: () => connection && void snooze(connection, ids, null),
          }
        : undefined,
    );

  return (
    <>
      <Text style={styles.title} numberOfLines={1}>
        {snoozed ? t('{title} is snoozed', { title: shown.title }) : t('Snooze {title}', { title: shown.title })}
      </Text>
      {snoozed ? (
        <View style={styles.items}>
          <MenuItem icon="IconClock" label={t('Wake now')} onPress={() => snoozeUntil(null)} />
        </View>
      ) : (
        <>
          {(presets.data ?? []).map((preset) => (
            <Pressable
              key={preset.label}
              onPress={() => snoozeUntil(preset.until)}
              style={({ pressed }) => [styles.time, pressed && { backgroundColor: colors.active }]}
              accessibilityRole="button">
              <Text style={styles.timeLabel}>{preset.label}</Text>
              <Text style={styles.timeWhen}>{clock(preset.until)}</Text>
            </Pressable>
          ))}
          <TextInput
            value={typed}
            onChangeText={(text) => {
              setTyped(text);
              setFailed(undefined);
            }}
            onSubmitEditing={() => typed.trim() && snoozeUntil(null, typed.trim())}
            placeholder={t('Or type a time: fri 2pm, in 90m')}
            placeholderTextColor={colors.tertiary}
            returnKeyType="done"
            autoCorrect={false}
            autoCapitalize="none"
            style={styles.field}
          />
        </>
      )}
      {failed ? <Text style={styles.failed}>{failed}</Text> : null}
      <View style={styles.items}>
        {settled ? (
          <MenuItem icon="IconRotate" label={t('Restore')} onPress={() => act((connection) => settle(connection, ids, false))} />
        ) : (
          <MenuItem
            icon="IconCheck"
            label={t('Settle')}
            onPress={() =>
              act((connection) => settle(connection, ids, true), {
                text: t('Settled'),
                undo: () => connection && void settle(connection, ids, false),
              })
            }
          />
        )}
      </View>
    </>
  );
}

/** Says what was just snoozed or settled from this phone, with Undo, for a few seconds. */
export function AwayNote() {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const shown = useExternalValue(note);
  const t = useT();
  if (!shown) return null;
  return (
    <View style={styles.note} accessibilityLiveRegion="polite">
      <Text style={styles.noteText}>{shown.text}</Text>
      <Pressable
        onPress={() => {
          haptics.tap();
          shown.undo();
          note.set(undefined);
        }}
        hitSlop={8}
        style={({ pressed }) => [styles.undo, pressed && { backgroundColor: colors.active }]}
        accessibilityRole="button">
        <Text style={styles.undoText}>{t('Undo')}</Text>
      </Pressable>
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    title: {
      fontFamily: fonts.uiSemibold,
      fontSize: 16,
      letterSpacing: -0.25,
      color: colors.ink,
      paddingHorizontal: 8,
      paddingTop: 4,
      paddingBottom: 10,
    },
    items: { gap: 8, paddingTop: 8, paddingBottom: 4 },
    time: { flexDirection: 'row', alignItems: 'center', height: 48, paddingHorizontal: 10, borderRadius: 9 },
    timeLabel: { flex: 1, fontFamily: fonts.ui, fontSize: 15, color: colors.inkDim },
    timeWhen: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    field: {
      height: 44,
      marginTop: 8,
      paddingHorizontal: 14,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
      fontFamily: fonts.ui,
      fontSize: 15,
      color: colors.ink,
    },
    failed: { paddingHorizontal: 8, paddingTop: 8, fontFamily: fonts.ui, fontSize: 13, color: colors.danger },
    note: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      alignSelf: 'flex-start',
      height: 36,
      marginTop: 10,
      paddingLeft: 12,
      paddingRight: 4,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    noteText: { fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink },
    undo: { height: 28, paddingHorizontal: 10, borderRadius: 7, justifyContent: 'center' },
    undoText: { fontFamily: fonts.uiSemibold, fontSize: 13.5, color: colors.ink },
  });
