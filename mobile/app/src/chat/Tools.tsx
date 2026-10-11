import { memo, useEffect, useState, type ReactNode } from 'react';
import { Image, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { collapseDiff } from '@mac/chat/diff';
import { durationLabel, elapsedLabel } from '@mac/chat/durationLabel';
import { toolDescription } from '@mac/chat/toolOutput';
import { toolDetail, toolKind, toolLabel, toolRunning, toolTarget } from '@mac/chat/toolLabels';
import { sikemuxToolRow, type ToolRowIcon } from '@mac/chat/toolRows';
import type { AcpToolCall, ChatPart } from '@mac/chat/types';
import { CopyButton } from '@/ui/CopyButton';
import { Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { Shimmer } from '@/ui/Shimmer';
import { useFold } from './folds';

export type ToolPart = Extract<ChatPart, { kind: 'tool' }>;

/** Lines of a change shown under its row; the host's diff view has the rest. */
const DIFF_ROWS = 120;
/** Lines of output shown before "Show all", as on the Mac. */
const OUTPUT_FOLD_LINES = 12;
/** JetBrains Mono's advance at the rows' 11.5px, for lining the verbs up in one column. */
const KIND_CHAR = 6.9;

const ROW_ICONS: Record<ToolRowIcon, IconName> = {
  activity: 'IconActivity',
  book: 'IconBook',
  camera: 'IconCamera',
  click: 'IconClick',
  clock: 'IconClock',
  close: 'IconClose',
  code: 'IconCode',
  dashboard: 'IconDashboard',
  download: 'IconDownload',
  eye: 'IconEye',
  folder: 'IconFolder',
  globe: 'IconGlobe',
  highlighter: 'IconHighlighter',
  'home-button': 'IconHomeButton',
  hourglass: 'IconHourglass',
  issue: 'IconIssue',
  keyboard: 'IconKeyboard',
  link: 'IconLink',
  'log-lines': 'IconLogLines',
  message: 'IconMessage',
  mouse: 'IconMouse',
  network: 'IconNetwork',
  panel: 'IconPanel',
  phone: 'IconPhone',
  pinch: 'IconPinch',
  pointer: 'IconPointer',
  'pull-request': 'IconPullRequest',
  record: 'IconRecord',
  refresh: 'IconRefresh',
  resize: 'IconResize',
  rocket: 'IconRocket',
  rotate: 'IconRotate',
  run: 'IconRun',
  search: 'IconSearch',
  'square-plus': 'IconSquarePlus',
  status: 'IconStatus',
  steps: 'IconSteps',
  stop: 'IconStop',
  swipe: 'IconSwipe',
  tabs: 'IconTabs',
  tap: 'IconTap',
  terminal: 'IconTerminal',
  text: 'IconText',
  'touch-path': 'IconTouchPath',
  unlink: 'IconUnlink',
  upload: 'IconUpload',
  waterfall: 'IconWaterfall',
  window: 'IconWindow',
  workflow: 'IconWorkflow',
};

/** Our own tools take the kind they declare; another server's call is named for the server it went to. */
function rowKind(tool: AcpToolCall): string | undefined {
  return sikemuxToolRow(tool)?.kind ?? (toolLabel(tool.title).scope !== undefined ? 'mcp' : tool.kind);
}

/** The icon and colour of a call, as the Mac's tool rows draw them. */
function look(tool: AcpToolCall, colors: Palette): { icon: IconName; color: string } {
  if (tool.status === 'failed') return { icon: 'IconWarning', color: colors.danger };
  const row = sikemuxToolRow(tool);
  const kind = rowKind(tool);
  const color =
    kind === 'read' || kind === 'search' || kind === 'fetch'
      ? colors.toolRead
      : kind === 'edit' || kind === 'move'
        ? colors.toolEdit
        : kind === 'delete'
          ? colors.toolDelete
          : kind === 'execute'
            ? colors.toolRun
            : kind === 'mcp'
              ? colors.toolMcp
              : colors.inkDim;
  if (row) return { icon: ROW_ICONS[row.icon], color };
  switch (kind) {
    case 'mcp':
      return { icon: 'IconPlug', color };
    case 'read':
      return { icon: 'IconFile', color };
    case 'search':
      return { icon: 'IconSearch', color };
    case 'edit':
    case 'move':
    case 'delete':
      return { icon: 'IconPencil', color };
    case 'execute':
      return { icon: 'IconCommand', color };
    case 'fetch':
      return { icon: 'IconGlobe', color };
    default:
      return { icon: 'IconAgent', color };
  }
}

/** Counts up in whole seconds on a clock of its own, so the tick redraws this label and nothing around it. */
export function LiveSeconds({ since, spent = 0, style }: { since?: number; spent?: number; style: object }) {
  const [started] = useState(() => since ?? Date.now());
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return <Text style={style}>{elapsedLabel(Math.max(0, Math.floor((spent + now - started) / 1000)))}</Text>;
}

/** Scrolls sideways inside the transcript, so long lines keep to the width of the screen. */
function Sideways({ children }: { children: ReactNode }) {
  const styles = useStyles(makeStyles);
  return (
    <ScrollView horizontal contentContainerStyle={styles.pad} showsHorizontalScrollIndicator={false}>
      {children}
    </ScrollView>
  );
}

/** A command and what it printed, the way a terminal would have shown them. */
function Terminal({ part, command, group }: { part: ToolPart; command: string | null; group: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const output = part.output;
  const [whole, setWhole] = useFold(`${group}/${part.id}/whole`, false);
  const lines = output?.text ? output.text.split('\n') : [];
  const folds = lines.length > OUTPUT_FOLD_LINES;
  const folded = folds && !whole;
  const failed = part.tool.status === 'failed';
  return (
    <View style={styles.box}>
      {command !== null ? (
        <View style={styles.command}>
          <Text style={[styles.mono, { color: colors.toolRun }]}>$</Text>
          <Text style={[styles.mono, styles.commandText]} selectable>
            {command}
          </Text>
          <CopyButton value={command} label="command" size={12} />
        </View>
      ) : null}
      {output?.image ? (
        <Image
          source={{ uri: `data:${output.image.mimeType};base64,${output.image.data}` }}
          style={styles.image}
          resizeMode="contain"
          accessibilityIgnoresInvertColors
        />
      ) : null}
      {output?.text ? (
        <View style={[styles.output, command !== null && styles.ruled]}>
          {folded ? (
            <Sideways>
              <Text style={[styles.mono, styles.outputText]}>{lines.slice(0, OUTPUT_FOLD_LINES).join('\n')}</Text>
            </Sideways>
          ) : (
            <ScrollView style={styles.whole} nestedScrollEnabled>
              <Sideways>
                <Text style={[styles.mono, styles.outputText]} selectable>
                  {output.text}
                </Text>
              </Sideways>
            </ScrollView>
          )}
          <CopyButton value={output.text} label="output" size={12} style={styles.copyOutput} />
        </View>
      ) : output && !output.image ? (
        <Text style={[styles.note, command !== null && styles.ruled]}>No output</Text>
      ) : null}
      {folds ? (
        <Pressable onPress={() => setWhole(!whole)} style={[styles.more, styles.ruled]} hitSlop={4} accessibilityRole="button">
          <Text style={styles.moreText}>{folded ? `Show all ${lines.length} lines` : 'Show fewer lines'}</Text>
        </Pressable>
      ) : null}
      {output?.cut && (!folds || whole) ? <Text style={[styles.note, styles.ruled]}>The rest of the output was not kept</Text> : null}
      {failed && output?.exitCode !== undefined ? <Text style={styles.exit}>exit {output.exitCode}</Text> : null}
    </View>
  );
}

function Diff({ part }: { part: ToolPart }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const diff = part.diff ? collapseDiff(part.diff.lines, 2) : null;
  if (!diff) return null;
  const shown = diff.rows.slice(0, DIFF_ROWS);
  const more = diff.rows.length - shown.length;
  return (
    <ScrollView style={[styles.box, styles.whole]} nestedScrollEnabled>
      <Sideways>
        <View>
          {shown.map((row, index) =>
            'gap' in row ? (
              <Text key={index} style={[styles.mono, styles.outputText, { color: colors.inkFaint }]}>
                ⋯ {row.gap} unchanged line{row.gap === 1 ? '' : 's'}
              </Text>
            ) : (
              <Text
                key={index}
                style={[
                  styles.mono,
                  styles.outputText,
                  row.sign === '+' && { color: colors.gitAdded },
                  row.sign === '-' && { color: colors.gitDeleted },
                ]}
                selectable>
                {row.sign} {row.text}
              </Text>
            ),
          )}
          {more > 0 ? (
            <Text style={[styles.mono, styles.outputText, { color: colors.inkFaint }]}>{more} more lines on the host</Text>
          ) : null}
        </View>
      </Sideways>
    </ScrollView>
  );
}

function ToolDetail({ part, command, group }: { part: ToolPart; command: string | null; group: string }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.detail}>
      {part.diff ? (
        <Diff part={part} />
      ) : command !== null || part.output ? (
        <Terminal part={part} command={command} group={group} />
      ) : (
        <Text style={styles.failure} selectable>
          {part.failure}
        </Text>
      )}
    </View>
  );
}

const ToolRow = memo(function ToolRow({
  part,
  last,
  untimed,
  group,
  kindWidth,
}: {
  part: ToolPart;
  last: boolean;
  untimed: boolean;
  group: string;
  kindWidth: number;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const tool = part.tool;
  const { icon, color } = look(tool, colors);
  const status = tool.status ?? 'pending';
  const running = toolRunning(tool);
  const target = toolTarget(tool);
  const detail = toolDetail(tool);
  const command = tool.kind === 'execute' ? tool.title.trim() : null;
  const opens = Boolean(part.diff || part.output || part.failure) || (command !== null && !running && command !== target);
  const [open, setOpen] = useFold(`${group}/${part.id}`, false);
  const measured = !untimed && part.startedAt !== undefined && part.endedAt !== undefined ? part.endedAt - part.startedAt : null;
  // A call the turn cut off has a duration, but printing it would read as a call that ran that long and then finished.
  const elapsed = status === 'cancelled' ? 'stopped' : measured !== null ? durationLabel(measured) : null;
  return (
    <View style={status === 'cancelled' && styles.cancelled}>
      <Pressable
        style={styles.row}
        onPress={opens ? () => setOpen(!open) : undefined}
        disabled={!opens}
        accessibilityRole={opens ? 'button' : undefined}
        accessibilityLabel={`${toolKind(tool)} ${target}${status === 'failed' ? ', failed' : running ? ', running' : ''}`}
        accessibilityState={opens ? { expanded: open } : undefined}>
        <View style={last && !open ? styles.elbow : styles.spine} />
        <View style={[styles.tick, last && !open && { backgroundColor: 'transparent' }]} />
        <Icon name={icon} size={12} color={color} />
        <Text style={[styles.mono, { color, minWidth: kindWidth }]} numberOfLines={1}>
          {toolKind(tool)}
        </Text>
        {running ? (
          <Shimmer style={[styles.mono, { color: colors.inkDim }]} layout={styles.targetLayout}>
            {target}
            {detail ? ` ${detail}` : null}
          </Shimmer>
        ) : (
          <Text style={[styles.mono, styles.target]} numberOfLines={1}>
            {target}
            {detail ? <Text style={{ color: colors.inkDim }}> {detail}</Text> : null}
          </Text>
        )}
        <View style={styles.end}>
          {part.diff ? (
            <Text style={styles.endText}>
              <Text style={{ color: colors.gitAdded }}>+{part.diff.adds}</Text>{' '}
              <Text style={{ color: colors.gitDeleted }}>−{part.diff.dels}</Text>
            </Text>
          ) : null}
          {running ? (
            <LiveSeconds since={part.startedAt} style={styles.endText} />
          ) : elapsed ? (
            <Text style={styles.endText}>{elapsed}</Text>
          ) : null}
        </View>
      </Pressable>
      {open ? (
        <View style={styles.detailRow}>
          {last ? null : <View style={styles.spine} />}
          <ToolDetail part={part} command={command} group={group} />
        </View>
      ) : null}
    </View>
  );
});

/* A message's runs of calls are gathered afresh on each update, so a group compares its calls rather than its array:
   while a reply streams, a group whose calls did not change is not drawn again. */
const sameCalls = (a: ToolPart[], b: ToolPart[]) => a.length === b.length && a.every((part, index) => part === b[index]);

/**
 * A run of calls stays open while the agent is still adding to it, as on the Mac, and folds once
 * something follows it or the turn ends; watching each call instead shuts it in the gaps between them.
 * While a call runs, the header says what the agent said it is for.
 */
export const ToolGroup = memo(
  function ToolGroup({ id, parts, untimed, live }: { id: string; parts: ToolPart[]; untimed: boolean; live: boolean }) {
    const colors = useColors();
    const styles = useStyles(makeStyles);
    const current = parts.find((part) => toolRunning(part.tool));
    const [open, setOpen] = useFold(id, live || current !== undefined);
    const spent = parts.reduce(
      (total, part) => total + (part.startedAt !== undefined && part.endedAt !== undefined ? part.endedAt - part.startedAt : 0),
      0,
    );
    const said = current ? toolDescription(current.tool) : null;
    const calls = `${parts.length} tool call${parts.length === 1 ? '' : 's'}`;
    const kindWidth = Math.min(16, Math.max(4, ...parts.map((part) => toolKind(part.tool).length))) * KIND_CHAR + 1;
    return (
      <View style={styles.group}>
        <Pressable
          onPress={() => setOpen(!open)}
          style={styles.summary}
          hitSlop={6}
          accessibilityRole="button"
          accessibilityLabel={`${said ?? calls}${current ? ', running' : ''}`}
          accessibilityState={{ expanded: open }}>
          {current ? (
            <Shimmer style={[styles.summaryText, said !== null && { color: colors.secondary }]} layout={styles.summaryLabel}>
              {said ?? calls}
            </Shimmer>
          ) : (
            <Text style={styles.summaryText} numberOfLines={1}>
              {calls}
            </Text>
          )}
          {said !== null && parts.length > 1 ? <Text style={styles.summaryTime}>{parts.length} calls</Text> : null}
          {current ? (
            <LiveSeconds key={current.id} since={current.startedAt} spent={spent} style={styles.summaryTime} />
          ) : !untimed && spent > 0 ? (
            <Text style={styles.summaryTime}>{durationLabel(spent)}</Text>
          ) : null}
          <View style={[styles.chevron, open && { transform: [{ rotate: '90deg' }] }]}>
            <Icon name="IconChevron" size={11} color={colors.inkDim} />
          </View>
        </Pressable>
        {open ? (
          <View style={styles.body}>
            {parts.map((part, index) => (
              <ToolRow key={part.id} part={part} last={index === parts.length - 1} untimed={untimed} group={id} kindWidth={kindWidth} />
            ))}
          </View>
        ) : null}
      </View>
    );
  },
  (before, after) =>
    before.id === after.id && before.untimed === after.untimed && before.live === after.live && sameCalls(before.parts, after.parts),
);

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    group: { marginVertical: 6 },
    summary: { flexDirection: 'row', alignItems: 'center', gap: 8, minHeight: 24, alignSelf: 'flex-start', maxWidth: '100%' },
    summaryText: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    summaryLabel: { flexShrink: 1 },
    summaryTime: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkDim },
    chevron: { opacity: 0.7 },
    body: { marginLeft: 6 },
    row: { flexDirection: 'row', alignItems: 'center', minHeight: 22, gap: 8 },
    cancelled: { opacity: 0.6 },
    spine: { position: 'absolute', left: 0, top: 0, bottom: 0, width: 1, backgroundColor: colors.treeSpine },
    // The last row's spine bends into its tick, ending on the row's middle like the Mac's.
    elbow: {
      position: 'absolute',
      left: 0,
      top: 0,
      bottom: '50%',
      width: 10,
      marginBottom: -0.5,
      borderLeftWidth: 1,
      borderBottomWidth: 1,
      borderBottomLeftRadius: 4,
      borderLeftColor: colors.treeSpine,
      borderBottomColor: colors.treeTick,
    },
    tick: { width: 10, height: 1, backgroundColor: colors.treeTick },
    mono: { fontFamily: fonts.mono, fontSize: 11.5 },
    target: { flex: 1, color: colors.ink },
    targetLayout: { flex: 1 },
    end: { flexDirection: 'row', gap: 6 },
    endText: { fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkDim },
    detailRow: { flexDirection: 'row' },
    detail: { flex: 1, marginLeft: 18, marginTop: 2, marginBottom: 6 },
    failure: { fontFamily: fonts.mono, fontSize: 11, lineHeight: 16, color: colors.danger },
    box: { borderRadius: 8, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.sunken, overflow: 'hidden' },
    whole: { maxHeight: 320 },
    pad: { paddingHorizontal: 9, paddingVertical: 7 },
    command: { flexDirection: 'row', alignItems: 'flex-start', gap: 7, paddingLeft: 9, paddingRight: 3, paddingVertical: 3 },
    commandText: { flex: 1, paddingVertical: 4, lineHeight: 17, color: colors.ink },
    output: { position: 'relative' },
    ruled: { borderTopWidth: 1, borderTopColor: colors.border },
    outputText: { fontSize: 11, lineHeight: 16, color: colors.inkDim, paddingRight: 24 },
    copyOutput: { position: 'absolute', top: 3, right: 3 },
    note: { paddingHorizontal: 9, paddingVertical: 6, fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint },
    more: { paddingHorizontal: 9, paddingVertical: 7 },
    moreText: { fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint },
    exit: { paddingHorizontal: 9, paddingBottom: 7, fontFamily: fonts.mono, fontSize: 10.5, color: colors.danger },
    image: { width: '100%', aspectRatio: 4 / 3, backgroundColor: colors.sunken },
  });
};
