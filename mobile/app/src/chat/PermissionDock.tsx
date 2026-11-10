import { useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { toolKind } from '@shared/chat/toolLabels';
import type { PermissionAsk, ToolCall } from '@shared/wire.generated';
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

/** One button of a dock: `go` is the one it leads with, `quiet` one that says no. */
export type DockAct = { key: string; label: string; go?: boolean; quiet?: boolean; wide?: boolean; onPress: () => void };

/**
 * Sits above the composer until it is answered: a title, the exact command and the answers, with
 * a pager when more than one waits. Wide answers take a row of their own above the rest.
 */
export function Dock({
  title,
  detail,
  label,
  pager,
  command,
  run,
  diff,
  acts,
  busy,
}: {
  title: string;
  detail: string;
  label: string;
  pager?: { index: number; count: number; onPage: (index: number) => void; noun: string };
  command: string;
  run: boolean;
  diff?: PermissionAsk['diff'];
  acts: DockAct[];
  busy: boolean;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const act = (each: DockAct) => (
    <Pressable
      key={each.key}
      disabled={busy}
      onPress={() => {
        haptics.select();
        each.onPress();
      }}
      style={({ pressed }) => [styles.act, each.go && styles.actGo, pressed && { opacity: 0.8 }, busy && { opacity: 0.5 }]}
      accessibilityRole="button"
      accessibilityState={{ disabled: busy, busy }}>
      <Text style={[styles.actText, each.go && styles.actGoText, each.quiet && { color: colors.secondary }]} numberOfLines={1}>
        {each.label}
      </Text>
    </Pressable>
  );
  const wide = acts.filter((each) => each.wide);
  return (
    <View style={styles.dock} accessibilityRole="alert" accessibilityLabel={label}>
      <View style={styles.dockHead}>
        <Icon name="IconShieldBolt" size={16} color={colors.ink} />
        <View style={{ flex: 1 }}>
          <Text style={styles.dockTitle}>{title}</Text>
          <Text style={styles.dockDetail}>{detail}</Text>
        </View>
        {pager && pager.count > 1 ? (
          <View style={styles.pages}>
            <Pressable
              onPress={() => pager.onPage(pager.index - 1)}
              disabled={pager.index === 0}
              hitSlop={8}
              style={[styles.pageButton, pager.index === 0 && { opacity: 0.3 }]}
              accessibilityRole="button"
              accessibilityLabel={`Previous ${pager.noun}`}>
              <View style={{ transform: [{ rotate: '180deg' }] }}>
                <Icon name="IconChevron" size={12} color={colors.secondary} />
              </View>
            </Pressable>
            <Text
              style={styles.pageText}
              accessibilityLabel={`${pager.noun[0].toUpperCase()}${pager.noun.slice(1)} ${pager.index + 1} of ${pager.count}`}>
              {pager.index + 1} of {pager.count}
            </Text>
            <Pressable
              onPress={() => pager.onPage(pager.index + 1)}
              disabled={pager.index === pager.count - 1}
              hitSlop={8}
              style={[styles.pageButton, pager.index === pager.count - 1 && { opacity: 0.3 }]}
              accessibilityRole="button"
              accessibilityLabel={`Next ${pager.noun}`}>
              <Icon name="IconChevron" size={12} color={colors.secondary} />
            </Pressable>
          </View>
        ) : null}
      </View>
      <ScrollView style={styles.dockCmd} contentContainerStyle={styles.dockCmdPad} nestedScrollEnabled showsVerticalScrollIndicator={false}>
        <Text style={styles.dockCmdText} selectable>
          {run ? <Text style={{ color: colors.toolRun }}>$ </Text> : null}
          {command}
        </Text>
        {diff ? (
          <Text style={[styles.dockCmdText, { marginTop: 4 }]}>
            <Text style={{ color: colors.gitAdded }}>+{diff.adds}</Text> <Text style={{ color: colors.gitDeleted }}>−{diff.dels}</Text>
          </Text>
        ) : null}
      </ScrollView>
      {wide.map((each) => (
        <View key={each.key} style={styles.dockActs}>
          {act(each)}
        </View>
      ))}
      <View style={styles.dockActs}>{acts.filter((each) => !each.wide).map(act)}</View>
    </View>
  );
}

/** A chat agent's permission requests, with the agent's own options, and Cancel when none of them says no. */
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
  const [page, setPage] = useState(0);
  const index = Math.min(page, requests.length - 1);
  const request = requests[index];
  const rejects = request.options.filter((option) => option.kind.startsWith('reject'));
  const always = request.options.filter((option) => option.kind === 'allow_always');
  const once = request.options.filter((option) => option.kind === 'allow_once');
  const cancel = rejects.length ? [] : [{ optionId: '', name: 'Cancel', kind: 'reject_once' }];
  const primary = once[0] ?? always[0];
  const title = askTitle(request);
  return (
    <Dock
      title={title}
      detail={`${agentLabel(provider)} needs permission to continue`}
      label={`${title} ${agentLabel(provider)} needs permission to continue`}
      pager={{ index, count: requests.length, onPage: setPage, noun: 'request' }}
      command={asked(request.toolCall)}
      run={toolKind(request.toolCall) === 'run'}
      diff={request.diff}
      busy={answering.has(request.requestId)}
      acts={[...cancel, ...rejects, ...always, ...once].map((option) => ({
        key: option.optionId || option.name,
        label: option.name,
        go: option === primary,
        quiet: option.kind.startsWith('reject'),
        onPress: () => onAnswer(request.requestId, option.optionId || null),
      }))}
    />
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
