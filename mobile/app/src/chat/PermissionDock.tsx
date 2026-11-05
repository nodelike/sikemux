import { useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { toolKind } from '@shared/chat/toolLabels';
import type { PermissionAsk, ToolCall } from '@shared/chat/types';
import { agentLabel } from '@shared/agents/catalog';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

export function askTitle(request: PermissionAsk): string {
  const kind = toolKind(request.toolCall);
  if (kind === 'run') return 'Run a command?';
  if (kind === 'edit' || kind === 'move' || kind === 'delete') return 'Change a file?';
  if (kind === 'fetch') return 'Open a web page?';
  return 'Allow this?';
}

/** All of what the agent asks to run or touch, since the end of a command can change what it does. */
export function asked(tool: ToolCall): string {
  const command = (tool.rawInput as { command?: unknown } | undefined)?.command;
  if (typeof command === 'string' && command.trim()) return command.trim();
  if (Array.isArray(command) && command.every((word) => typeof word === 'string')) return command.join(' ');
  return tool.row.path ?? tool.row.target;
}

/**
 * Sits above the composer until it is answered: the Mac's permission card, with the agent's own
 * options, and Cancel when none of them says no.
 */
export function PermissionDock({
  requests,
  provider,
  answering,
  onAnswer,
}: {
  requests: PermissionAsk[];
  provider: string;
  answering: ReadonlySet<string>;
  onAnswer: (requestId: string, optionId: string | null) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [page, setPage] = useState(0);
  const index = Math.min(page, requests.length - 1);
  const request = requests[index];
  const busy = answering.has(request.requestId);
  const rejects = request.options.filter((option) => option.kind.startsWith('reject'));
  const always = request.options.filter((option) => option.kind === 'allow_always');
  const once = request.options.filter((option) => option.kind === 'allow_once');
  const cancel = rejects.length ? [] : [{ optionId: '', name: 'Cancel', kind: 'reject_once' }];
  const ordered = [...cancel, ...rejects, ...always, ...once];
  const primary = once[0] ?? always[0];
  const run = toolKind(request.toolCall) === 'run';
  const title = askTitle(request);
  return (
    <View
      style={styles.dock}
      accessibilityRole="alert"
      accessibilityLabel={`${title} ${agentLabel(provider)} needs permission to continue`}>
      <View style={styles.dockHead}>
        <Icon name="IconShieldBolt" size={16} color={colors.ink} />
        <View style={{ flex: 1 }}>
          <Text style={styles.dockTitle}>{title}</Text>
          <Text style={styles.dockDetail}>{agentLabel(provider)} needs permission to continue</Text>
        </View>
        {requests.length > 1 ? (
          <View style={styles.pages}>
            <Pressable
              onPress={() => setPage(index - 1)}
              disabled={index === 0}
              hitSlop={8}
              style={[styles.pageButton, index === 0 && { opacity: 0.3 }]}
              accessibilityRole="button"
              accessibilityLabel="Previous request">
              <View style={{ transform: [{ rotate: '180deg' }] }}>
                <Icon name="IconChevron" size={12} color={colors.secondary} />
              </View>
            </Pressable>
            <Text style={styles.pageText} accessibilityLabel={`Request ${index + 1} of ${requests.length}`}>
              {index + 1} of {requests.length}
            </Text>
            <Pressable
              onPress={() => setPage(index + 1)}
              disabled={index === requests.length - 1}
              hitSlop={8}
              style={[styles.pageButton, index === requests.length - 1 && { opacity: 0.3 }]}
              accessibilityRole="button"
              accessibilityLabel="Next request">
              <Icon name="IconChevron" size={12} color={colors.secondary} />
            </Pressable>
          </View>
        ) : null}
      </View>
      <ScrollView style={styles.dockCmd} contentContainerStyle={styles.dockCmdPad} nestedScrollEnabled showsVerticalScrollIndicator={false}>
        <Text style={styles.dockCmdText} selectable>
          {run ? <Text style={{ color: colors.toolRun }}>$ </Text> : null}
          {asked(request.toolCall)}
        </Text>
        {request.diff ? (
          <Text style={[styles.dockCmdText, { marginTop: 4 }]}>
            <Text style={{ color: colors.gitAdded }}>+{request.diff.adds}</Text>{' '}
            <Text style={{ color: colors.gitDeleted }}>−{request.diff.dels}</Text>
          </Text>
        ) : null}
      </ScrollView>
      <View style={styles.dockActs}>
        {ordered.map((option) => {
          const go = option === primary;
          return (
            <Pressable
              key={option.optionId || option.name}
              disabled={busy}
              onPress={() => {
                haptics.select();
                onAnswer(request.requestId, option.optionId || null);
              }}
              style={({ pressed }) => [styles.act, go && styles.actGo, pressed && { opacity: 0.8 }, busy && { opacity: 0.5 }]}
              accessibilityRole="button"
              accessibilityState={{ disabled: busy, busy }}>
              <Text
                style={[styles.actText, go && styles.actGoText, option.kind.startsWith('reject') && { color: colors.secondary }]}
                numberOfLines={1}>
                {option.name}
              </Text>
            </Pressable>
          );
        })}
      </View>
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    pages: { flexDirection: 'row', alignItems: 'center', gap: 2, marginTop: -2 },
    pageButton: { width: 26, height: 26, borderRadius: 8, alignItems: 'center', justifyContent: 'center' },
    pageText: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    dock: {
      marginBottom: 8,
      padding: 12,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      borderRadius: 16,
      backgroundColor: colors.overlay,
    },
    dockHead: { flexDirection: 'row', alignItems: 'flex-start', gap: 10 },
    dockTitle: { fontFamily: fonts.uiSemibold, fontSize: 15, letterSpacing: -0.2, color: colors.ink },
    dockDetail: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary, marginTop: 1 },
    dockCmd: {
      marginTop: 10,
      maxHeight: 150,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
    },
    dockCmdPad: { paddingVertical: 9, paddingHorizontal: 11 },
    dockCmdText: { fontFamily: fonts.mono, fontSize: 12.5, lineHeight: 18, color: colors.ink },
    dockActs: { flexDirection: 'row', gap: 6, marginTop: 10 },
    act: {
      flex: 1,
      minHeight: 38,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      alignItems: 'center',
      justifyContent: 'center',
      paddingHorizontal: 6,
    },
    actGo: { backgroundColor: colors.ink, borderColor: colors.ink },
    actText: { fontFamily: fonts.uiMedium, fontSize: 14, color: colors.ink },
    actGoText: { fontFamily: fonts.uiSemibold, color: colors.ground },
  });
};
