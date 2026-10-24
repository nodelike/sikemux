import { createContext, memo, useContext, useEffect, useState, type ReactNode } from 'react';
import { Image, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { durationLabel } from '@mac/chat/durationLabel';
import { toolDetail, toolKind, toolPath, toolRunning, toolTarget, toolUrl } from '@mac/chat/toolLabels';
import { attachmentName } from '@mac/chat/transcript';
import {
  callCount,
  callsLabel,
  secondsSince,
  toolElapsed,
  toolGroupSummary,
  toolIconName,
  toolRowKind,
  type ToolIconName,
} from '@mac/chat/toolView';
import type { ChatPart, ToolCall, ToolDetail as Detail, ToolOutput } from '@mac/chat/types';
import { problem as problemOf } from '@/core/problem';
import { CopyButton } from '@/ui/CopyButton';
import { Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { openLink } from '@/ui/prompt';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { Shimmer } from '@/ui/Shimmer';
import { DiffBody } from './Diff';
import { FileRef } from './FileRef';
import { useFold } from './folds';
import { ImageViewerHost, useShowImage } from './ImageViewer';

export type ToolPart = Extract<ChatPart, { kind: 'tool' }>;

/**
 * Reads a call's whole change and output from the host, or null when it will not say; `done` says the call will not
 * change any more. Absent where nothing can read it.
 */
export const ToolDetailContext = createContext<((toolCallId: string, done: boolean) => Promise<Detail | null>) | null>(null);

type Fetched = { detail: Detail | null; problem: string | null };

/** What an opened row shows beyond its summary, fetched when it opens and again as a running call goes on. */
function useToolDetail(part: ToolPart): Fetched {
  const read = useContext(ToolDetailContext);
  const [fetched, setFetched] = useState<Fetched>({ detail: null, problem: null });
  const done = !toolRunning(part.tool);
  const id = part.tool.toolCallId;
  const lines = part.output?.lines;
  useEffect(() => {
    if (!read) return;
    let current = true;
    read(id, done).then(
      (detail) => current && setFetched(detail ? { detail, problem: null } : { detail: null, problem: 'the host did not send it' }),
      (error: unknown) => current && setFetched((before) => ({ ...before, problem: problemOf(error) })),
    );
    return () => {
      current = false;
    };
  }, [read, id, done, lines]);
  return read ? fetched : { detail: null, problem: 'the host is not here to send it' };
}

/** Lines of output shown before "Show all", as on the Mac. */
const OUTPUT_FOLD_LINES = 12;
/** JetBrains Mono's advance at the rows' 11.5px, for lining the verbs up in one column. */
const KIND_CHAR = 6.9;

const TOOL_ICONS: Record<ToolIconName, IconName> = {
  activity: 'IconActivity',
  agent: 'IconAgent',
  book: 'IconBook',
  camera: 'IconCamera',
  click: 'IconClick',
  clock: 'IconClock',
  close: 'IconClose',
  code: 'IconCode',
  command: 'IconCommand',
  dashboard: 'IconDashboard',
  download: 'IconDownload',
  eye: 'IconEye',
  file: 'IconFile',
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
  move: 'IconMove',
  network: 'IconNetwork',
  panel: 'IconPanel',
  pencil: 'IconPencil',
  phone: 'IconPhone',
  pinch: 'IconPinch',
  plug: 'IconPlug',
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
  trash: 'IconTrash',
  unlink: 'IconUnlink',
  upload: 'IconUpload',
  warning: 'IconWarning',
  waterfall: 'IconWaterfall',
  window: 'IconWindow',
  workflow: 'IconWorkflow',
  wrench: 'IconWrench',
};

/** The marks of the plugins whose tools the phone's icon set already draws; the rest keep the call's own icon. */
const PLUGIN_MARKS: Readonly<Record<string, IconName>> = { 'sikemux.aws': 'IconAws', 'sikemux.rundeck': 'IconRundeck' };

/** The colour of each kind of call; any other kind is drawn in `inkDim`. */
const KIND_COLORS: Readonly<Record<string, keyof Palette>> = {
  read: 'toolRead',
  search: 'toolRead',
  fetch: 'toolRead',
  edit: 'toolEdit',
  move: 'toolEdit',
  delete: 'toolDelete',
  execute: 'toolRun',
  mcp: 'toolMcp',
};

/** The icon and colour of a call, as the Mac's tool rows draw them. */
function look(tool: ToolCall, colors: Palette): { icon: IconName; color: string } {
  const plugin = tool.status === 'failed' ? undefined : tool.row.plugin;
  const icon = (plugin && PLUGIN_MARKS[plugin]) || TOOL_ICONS[toolIconName(tool)];
  if (tool.status === 'failed') return { icon, color: colors.danger };
  const kind = toolRowKind(tool);
  return { icon, color: colors[(kind && KIND_COLORS[kind]) || 'inkDim'] };
}

/** Counts up in whole seconds on a clock of its own, so the tick redraws this label and nothing around it. */
export function LiveSeconds({ since, spent = 0, style }: { since?: number; spent?: number; style: object }) {
  const [started] = useState(() => since ?? Date.now());
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return <Text style={style}>{secondsSince(started, now, spent)}</Text>;
}

/* While a view around a scroller holds the touch, iOS won't let the scroller pan, so the
   scroller's own content takes it before the message's press handler can. */
const keepTouch = () => true;

/** Scrolls sideways inside the transcript, so long lines keep to the width of the screen. */
function Sideways({ children }: { children: ReactNode }) {
  const styles = useStyles(makeStyles);
  return (
    <ScrollView horizontal contentContainerStyle={styles.pad} showsHorizontalScrollIndicator={false}>
      <View onStartShouldSetResponder={keepTouch}>{children}</View>
    </ScrollView>
  );
}

/** A command and what it printed, the way a terminal would have shown them. */
function Terminal({
  part,
  command,
  group,
  output,
  waiting,
}: {
  part: ToolPart;
  command: string | null;
  group: string;
  output: ToolOutput | undefined;
  /** Stands in for the output until the host has sent it. */
  waiting: string | null;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const show = useShowImage();
  const summary = part.output;
  const picture = output?.image
    ? { uri: `data:${output.image.mimeType};base64,${output.image.data}`, name: attachmentName(output.image.mimeType) }
    : null;
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
      {picture ? (
        <Pressable onPress={() => show(picture)} accessibilityRole="imagebutton" accessibilityLabel="Picture the call returned, open">
          <Image source={{ uri: picture.uri }} style={styles.image} resizeMode="contain" accessibilityIgnoresInvertColors />
        </Pressable>
      ) : null}
      {output?.text ? (
        <View style={[styles.output, command !== null && styles.ruled]}>
          {folded ? (
            <Sideways>
              <Text style={[styles.mono, styles.outputText]}>{lines.slice(0, OUTPUT_FOLD_LINES).join('\n')}</Text>
            </Sideways>
          ) : (
            <ScrollView style={styles.whole} nestedScrollEnabled showsVerticalScrollIndicator={false}>
              <Sideways>
                <Text style={[styles.mono, styles.outputText]} selectable>
                  {output.text}
                </Text>
              </Sideways>
            </ScrollView>
          )}
          <CopyButton value={output.text} label="output" size={12} style={styles.copyOutput} />
        </View>
      ) : waiting ? (
        <Text style={[styles.note, command !== null && styles.ruled]}>{waiting}</Text>
      ) : summary && !output?.image ? (
        <Text style={[styles.note, command !== null && styles.ruled]}>No output</Text>
      ) : null}
      {folds ? (
        <Pressable onPress={() => setWhole(!whole)} style={[styles.more, styles.ruled]} hitSlop={4} accessibilityRole="button">
          <Text style={styles.moreText}>{folded ? `Show all ${lines.length} lines` : 'Show fewer lines'}</Text>
        </Pressable>
      ) : null}
      {(output ?? summary)?.cut && (!folds || whole) ? (
        <Text style={[styles.note, styles.ruled]}>The rest of the output was not kept</Text>
      ) : null}
      {failed && summary?.exitCode !== undefined ? <Text style={styles.exit}>exit {summary.exitCode}</Text> : null}
    </View>
  );
}

function ToolDetail({ part, command, group }: { part: ToolPart; command: string | null; group: string }) {
  const styles = useStyles(makeStyles);
  const { detail, problem } = useToolDetail(part);
  const waiting = detail ? null : problem ? `Could not load it: ${problem}` : 'Loading…';
  return (
    <View style={styles.detail}>
      {part.diff ? (
        <DiffBody
          diff={detail?.diff ?? part.diff}
          fold={`${group}/${part.id}/diff`}
          waiting={detail?.diff ? undefined : (waiting ?? undefined)}
        />
      ) : command !== null || part.output ? (
        <Terminal part={part} command={command} group={group} output={detail?.output} waiting={part.output ? waiting : null} />
      ) : (
        <Text style={styles.failure} selectable>
          {part.failure}
        </Text>
      )}
    </View>
  );
}

/** What a call acted on: a file as the Mac's file ref, a web address as a link, anything else as written. */
function Target({ tool, target }: { tool: ToolCall; target: string }) {
  const styles = useStyles(makeStyles);
  const file = toolPath(tool);
  if (file) return <FileRef raw={file} label={target} look="plain" />;
  const link = toolUrl(target);
  if (!link) return target;
  return (
    <>
      {link.before}
      <Text style={styles.link} onPress={() => openLink(link.url)} accessibilityRole="link">
        {link.raw}
      </Text>
      {link.after}
    </>
  );
}

const ToolRow = memo(function ToolRow({
  part,
  last,
  group,
  kindWidth,
}: {
  part: ToolPart;
  last: boolean;
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
  // The row holds one line of a command, so the whole of it is worth opening while it runs or when the row shortened it.
  const opens = Boolean(part.diff || part.output || part.failure) || (command !== null && (running || command !== target));
  const [open, setOpen] = useFold(`${group}/${part.id}`, false);
  const elapsed = toolElapsed(part);
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
            <Target tool={tool} target={target} />
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
          {opens ? (
            <View style={[styles.rowChevron, open && styles.rowChevronOpen]}>
              <Icon name="IconChevron" size={10} color={colors.inkDim} />
            </View>
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
  function ToolGroup({ id, parts, live }: { id: string; parts: ToolPart[]; live: boolean }) {
    const colors = useColors();
    const styles = useStyles(makeStyles);
    const summary = toolGroupSummary(parts);
    const { current, said, spent } = summary;
    const [open, setOpen] = useFold(id, live || current !== undefined);
    const calls = callsLabel(parts.length);
    const kindWidth = summary.kindWidth * KIND_CHAR + 1;
    return (
      <ImageViewerHost>
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
            {said !== null && parts.length > 1 ? <Text style={styles.summaryTime}>{callCount(parts.length)}</Text> : null}
            {current ? (
              <LiveSeconds key={current.id} since={current.startedAt} spent={spent} style={styles.summaryTime} />
            ) : spent > 0 ? (
              <Text style={styles.summaryTime}>{durationLabel(spent)}</Text>
            ) : null}
            <View style={[styles.chevron, open && { transform: [{ rotate: '90deg' }] }]}>
              <Icon name="IconChevron" size={11} color={colors.inkDim} />
            </View>
          </Pressable>
          {open ? (
            <View style={styles.body}>
              {parts.map((part, index) => (
                <ToolRow key={part.id} part={part} last={index === parts.length - 1} group={id} kindWidth={kindWidth} />
              ))}
            </View>
          ) : null}
        </View>
      </ImageViewerHost>
    );
  },
  (before, after) => before.id === after.id && before.live === after.live && sameCalls(before.parts, after.parts),
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
    end: { flexDirection: 'row', alignItems: 'center', gap: 6 },
    link: { color: colors.accent },
    rowChevron: { opacity: 0.55 },
    rowChevronOpen: { opacity: 1, transform: [{ rotate: '90deg' }] },
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
