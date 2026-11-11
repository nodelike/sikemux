import { createContext, memo, useContext, useState } from 'react';
import { Image, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { durationLabel } from '@shared/chat/durationLabel';
import { useToolDetail } from '@shared/chat/useToolDetail';
import { toolKind, toolRunning, toolUrl } from '@shared/chat/toolLabels';
import { attachmentName } from '@shared/chat/transcript';
import { basename } from '@shared/lib/paths';
import {
  callCommand,
  callFailed,
  callOpens,
  callTook,
  lineNames,
  linesSayAll,
  lineVerb,
  placeOf,
  runCalls,
  runOf,
  sentence,
} from '@shared/chat/runs';
import { secondsSince, toolElapsed, toolIconName, type ToolIconName } from '@shared/chat/toolView';
import type { ChatPart, RunLine as Line, ToolCall, ToolDetail as Detail, ToolFamily, ToolIntent, ToolOutput } from '@shared/wire.generated';
import { problem as problemOf } from '@/core/problem';
import { CopyButton } from '@/ui/CopyButton';
import { Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { openLink } from '@/ui/prompt';
import { fonts, mixed, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { Shimmer } from '@/ui/Shimmer';
import { useSecond } from '@/ui/motion';
import { Sideways } from '@/ui/Sideways';
import { DiffBody } from './Diff';
import { FileRef, hostImagePath } from './FileRef';
import { useHostFile } from './hostFiles';
import { useFold } from './folds';
import { ImageViewerHost, useShowImage } from './ImageViewer';

export type ToolPart = Extract<ChatPart, { kind: 'tool' }>;

/**
 * Reads a call's whole change and output from the host, or null when it will not say; `done` says the call will not
 * change any more. Absent where nothing can read it.
 */
export const ToolDetailContext = createContext<((toolCallId: string, done: boolean) => Promise<Detail | null>) | null>(null);

/** What an opened row shows beyond its summary, and why it could not get it. */
function useToolFetch(part: ToolPart): { detail: Detail | null; problem: string | null } {
  const read = useContext(ToolDetailContext);
  const { detail, error } = useToolDetail(read, part);
  if (!read) return { detail: null, problem: 'the host is not here to send it' };
  return { detail, problem: error ? problemOf(error) : null };
}

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
  logo: 'Logo',
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
  const kind = tool.row.action;
  return { icon, color: colors[(kind && KIND_COLORS[kind]) || 'inkDim'] };
}

/** Counts up in whole seconds, so the tick redraws this label and nothing around it. */
export function LiveSeconds({ since, spent = 0, style }: { since?: number; spent?: number; style: object }) {
  const [started] = useState(() => since ?? Date.now());
  return <Text style={style}>{secondsSince(started, useSecond() * 1000, spent)}</Text>;
}

/** Lines of what a command printed shown before "Show all": its last ones, as on the Mac. */
const OUTPUT_TAIL_LINES = 8;

/** What a command printed: its exit code and size, then its last lines, the rest a tap away. */
function Terminal({
  part,
  command,
  group,
  output,
  waiting,
}: {
  part: ToolPart;
  /** The whole command, when its line had to cut it short. */
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
  const text = output?.text?.replace(/\n+$/, '') ?? '';
  const lines = text ? text.split('\n') : [];
  const folds = lines.length > OUTPUT_TAIL_LINES;
  const shown = folds && !whole ? lines.slice(-OUTPUT_TAIL_LINES).join('\n') : text;
  const exit = part.tool.status === 'failed' ? (summary?.exitCode !== undefined ? `exit ${summary.exitCode}` : 'failed') : null;
  return (
    <View>
      {command !== null ? (
        <View style={[styles.well, styles.command]}>
          <Text style={[styles.mono, { color: colors.toolRun }]}>$</Text>
          <Text style={[styles.mono, styles.commandText]} selectable>
            {command}
          </Text>
          <CopyButton value={command} label="command" size={12} />
        </View>
      ) : null}
      {exit || lines.length > 0 || folds ? (
        <View style={styles.outputHead}>
          {exit ? <Text style={styles.exit}>{exit}</Text> : null}
          {lines.length > 0 ? <Text style={styles.faint}>{`${lines.length} ${lines.length === 1 ? 'line' : 'lines'}`}</Text> : null}
          {folds ? (
            <Pressable onPress={() => setWhole(!whole)} style={styles.showAll} hitSlop={8} accessibilityRole="button">
              <View style={whole && styles.turned}>
                <Icon name="IconChevron" size={9} color={colors.inkDim} />
              </View>
              <Text style={styles.quiet}>{whole ? 'Show less' : 'Show all'}</Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
      {picture ? (
        <Pressable onPress={() => show(picture)} accessibilityRole="imagebutton" accessibilityLabel="Picture the call returned, open">
          <Image source={{ uri: picture.uri }} style={styles.image} resizeMode="contain" accessibilityIgnoresInvertColors />
        </Pressable>
      ) : null}
      {shown ? (
        <View style={styles.well}>
          {whole ? (
            <ScrollView style={styles.whole} nestedScrollEnabled showsVerticalScrollIndicator={false}>
              <Sideways contentStyle={styles.printedPad}>
                <Text style={[styles.mono, styles.printed]} selectable>
                  {shown}
                </Text>
              </Sideways>
            </ScrollView>
          ) : (
            <Sideways contentStyle={styles.printedPad}>
              <Text style={[styles.mono, styles.printed]}>{shown}</Text>
            </Sideways>
          )}
          <CopyButton value={text} label="output" size={12} style={styles.copyOutput} />
        </View>
      ) : waiting ? (
        <Text style={styles.note}>{waiting}</Text>
      ) : summary && !output?.image ? (
        <Text style={styles.note}>No output</Text>
      ) : null}
      {(output ?? summary)?.cut && (!folds || whole) ? <Text style={styles.note}>The rest of the output was not kept</Text> : null}
    </View>
  );
}

/** What an opened call did: its change, what it printed, or why it failed. */
function ToolDetail({ part, group }: { part: ToolPart; group: string }) {
  const styles = useStyles(makeStyles);
  const { detail, problem } = useToolFetch(part);
  const waiting = detail ? null : problem ? `Could not load it: ${problem}` : 'Loading…';
  const command = callCommand(part);
  return (
    <View style={styles.detail}>
      {part.diff ? (
        <DiffBody
          lines={detail?.diff?.lines}
          fold={`${group}/${part.id}/diff`}
          waiting={detail?.diff ? undefined : (waiting ?? undefined)}
        />
      ) : command !== null || part.output ? (
        <Terminal
          part={part}
          command={command !== null && command !== part.tool.row.target ? command : null}
          group={group}
          output={detail?.output}
          waiting={part.output ? waiting : null}
        />
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
  const file = tool.row.path;
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
  const target = tool.row.target;
  const detail = tool.row.detail;
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
          {tool.row.times ? <Text style={styles.endText}>×{tool.row.times}</Text> : null}
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
          <View style={styles.rowDetail}>
            <ToolDetail part={part} group={group} />
          </View>
        </View>
      ) : null}
    </View>
  );
});

/** The icon of a line or a strip, by what it did and the kind of tool it is, as the Mac's `line_icon`. */
function lineIcon(intent: ToolIntent, family: ToolFamily, call?: ToolPart): IconName {
  const plugin = call?.tool.row.plugin;
  if (plugin && PLUGIN_MARKS[plugin]) return PLUGIN_MARKS[plugin];
  const simulator = family === 'simulator';
  switch (intent) {
    case 'read':
      return family === 'browser' ? 'IconEye' : family === 'web' ? 'IconGlobe' : 'IconFile';
    case 'fetch':
      return 'IconGlobe';
    case 'view':
      return 'IconImage';
    case 'search':
      return family === 'web' ? 'IconGlobe' : 'IconSearch';
    case 'list':
      return 'IconFolder';
    case 'edit':
    case 'write':
      return 'IconPencil';
    case 'move':
      return 'IconMove';
    case 'delete':
      return 'IconTrash';
    case 'run':
    case 'test':
    case 'build':
      return 'IconCommand';
    case 'git':
    case 'commit':
      return 'IconGit';
    case 'open':
      return simulator ? 'IconPhone' : 'IconGlobe';
    case 'click':
      return simulator ? 'IconTap' : 'IconClick';
    case 'type':
      return 'IconKeyboard';
    case 'look':
      return 'IconCamera';
    case 'message':
      return 'IconMessage';
    case 'start':
      return family === 'shell' ? 'IconRun' : 'IconAgent';
    case 'stop':
      return 'IconStop';
    case 'wait':
      return 'IconClock';
    case 'mode':
      return 'IconSettings';
    case 'query':
    case 'use':
      return 'IconPlug';
  }
}

/** The colour of what a line did: looking, changing, removing, running or calling an agent. */
function intentTint(intent: ToolIntent, colors: Palette): string {
  const dim = colors.inkDim;
  switch (intent) {
    case 'read':
    case 'view':
    case 'search':
    case 'list':
    case 'fetch':
    case 'open':
    case 'look':
    case 'click':
    case 'type':
      return mixed(colors.gitRenamed, dim, 0.76);
    case 'edit':
    case 'write':
    case 'move':
    case 'commit':
      return mixed(colors.gitModified, dim, 0.76);
    case 'delete':
      return mixed(colors.gitDeleted, dim, 0.76);
    case 'run':
    case 'test':
    case 'build':
    case 'git':
    case 'start':
    case 'stop':
      return mixed(colors.cmd, dim, 0.7);
    case 'message':
      return mixed(colors.live, dim, 0.66);
    default:
      return dim;
  }
}

function Chevron({ open, size = 11 }: { open: boolean; size?: number }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={open && styles.turned}>
      <Icon name="IconChevron" size={size} color={colors.inkDim} />
    </View>
  );
}

/**
 * One thing the run did: a command, a change, or neighbours that did the same thing named
 * together. A line of one call opens onto what it changed or printed.
 */
const RunLine = memo(function RunLine({ line, run, group }: { line: Line; run: ToolPart[]; group: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const calls = line.calls.flatMap((at) => run[at] ?? []);
  const first = calls[0];
  const lone = calls.length === 1 ? first : undefined;
  const opens = lone !== undefined && callOpens(lone);
  const [open, setOpen] = useFold(`${group}/${first?.id}`, false);
  if (!first) return null;
  const running = calls.some((call) => toolRunning(call.tool));
  const tint = line.failed ? colors.danger : intentTint(line.intent, colors);
  const saidByIntent = ['read', 'view', 'search', 'list'].includes(line.intent) && first.tool.row.subject !== undefined;
  const command = lone && callCommand(lone) !== null && !saidByIntent ? lone.tool.row.target : null;
  const within = first.tool.row.within;
  const { names, more } = lineNames(calls);
  const took = calls.reduce((sum, call) => sum + callTook(call), 0);
  const diff = lone?.diff;
  const label = command ?? `${lineVerb(line, names.length > 0)} ${names.map(({ name }) => name).join(', ')}`;
  return (
    <View>
      <Pressable
        onPress={opens ? () => setOpen(!open) : undefined}
        disabled={!opens}
        style={styles.line}
        accessibilityRole={opens ? 'button' : undefined}
        accessibilityLabel={`${label}${line.failed ? ', failed' : running ? ', running' : ''}`}
        accessibilityState={opens ? { expanded: open } : undefined}>
        <View style={styles.lineIcon}>
          <Icon name={lineIcon(line.intent, line.family, first)} size={12} color={tint} />
        </View>
        {command !== null ? (
          running ? (
            <Shimmer style={[styles.mono, styles.typed]} layout={styles.lineMain}>
              {command}
            </Shimmer>
          ) : (
            <Text style={[styles.mono, styles.typed, styles.lineMain, line.failed && { color: colors.danger }]} numberOfLines={1}>
              {command}
            </Text>
          )
        ) : (
          <View style={[styles.lineNamed, styles.lineMain]}>
            <Text style={styles.lineWords}>{lineVerb(line, names.length > 0)}</Text>
            {names.map(({ name, path }) =>
              path ? (
                <FileChip key={name} name={name} path={path} withLines={line.intent !== 'view'} />
              ) : (
                <Text key={name} style={styles.name} numberOfLines={1}>
                  {name}
                </Text>
              ),
            )}
            {more > 0 ? <Text style={styles.lineWords}>{`+${more}`}</Text> : null}
            {within ? (
              <Text style={styles.lineWithin} numberOfLines={1}>
                <Text style={styles.lineWords}>in </Text>
                {within}
              </Text>
            ) : null}
          </View>
        )}
        <View style={styles.lineEnd}>
          {diff ? (
            <>
              <Text style={[styles.endText, { color: colors.gitAdded }]}>+{diff.adds}</Text>
              <Text style={[styles.endText, { color: colors.gitDeleted }]}>−{diff.dels}</Text>
            </>
          ) : null}
          {calls.length > 1 ? <Text style={styles.endText}>{`${calls.length} calls`}</Text> : null}
          {command !== null && !running ? (
            <Icon name={line.failed ? 'IconClose' : 'IconCheck'} size={11} color={line.failed ? colors.danger : colors.gitAdded} />
          ) : null}
          {running ? (
            <LiveSeconds since={first.startedAt} style={styles.endText} />
          ) : took > 0 ? (
            <Text style={styles.endText}>{durationLabel(took)}</Text>
          ) : null}
        </View>
      </Pressable>
      {line.failed && first.failure && !open ? (
        <Text style={styles.lineFailure} numberOfLines={3}>
          {first.failure}
        </Text>
      ) : null}
      {line.intent === 'view' ? <Pictures calls={calls} /> : null}
      {open && lone ? (
        <View style={styles.lineDetail}>
          <ToolDetail part={lone} group={group} />
        </View>
      ) : null}
    </View>
  );
});

/** A file a line names: its glyph, its name and the lines it touched. Only a picture opens; the phone has no editor. */
function FileChip({ name, path, withLines }: { name: string; path: string; withLines: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const show = useShowImage();
  const { file, lines } = placeOf(path);
  const image = hostImagePath(file);
  return (
    <Pressable
      onPress={image ? () => show({ path: image, name: basename(image) }) : undefined}
      disabled={!image}
      style={styles.chip}
      accessibilityRole={image ? 'imagebutton' : undefined}
      accessibilityLabel={`${name}${withLines && lines ? `, lines ${lines}` : ''}`}>
      <Icon name="IconFile" size={11} color={colors.inkDim} />
      <Text style={styles.chipName} numberOfLines={1}>
        {name}
      </Text>
      {withLines && lines ? <Text style={styles.chipLines}>{lines}</Text> : null}
    </Pressable>
  );
}

/** The most pictures a line shows of those its calls looked at. */
const LINE_PICTURES = 6;

/** The pictures a line's calls looked at on the host, small, with their names. */
function Pictures({ calls }: { calls: ToolPart[] }) {
  const styles = useStyles(makeStyles);
  const paths = calls
    .flatMap((call) => (call.tool.row.path ? [hostImagePath(placeOf(call.tool.row.path).file)] : []))
    .filter((path): path is string => path !== null)
    .slice(0, LINE_PICTURES);
  if (!paths.length) return null;
  return (
    <View style={styles.pictures}>
      {paths.map((path) => (
        <Thumbnail key={path} path={path} />
      ))}
    </View>
  );
}

function Thumbnail({ path }: { path: string }) {
  const styles = useStyles(makeStyles);
  const show = useShowImage();
  const uri = useHostFile(path, true, THUMB_EDGE);
  const name = basename(path);
  return (
    <Pressable
      onPress={() => show({ path, name })}
      style={styles.thumb}
      accessibilityRole="imagebutton"
      accessibilityLabel={`${name}, open`}>
      {uri ? (
        <Image source={{ uri }} style={styles.thumbImage} resizeMode="cover" accessibilityIgnoresInvertColors />
      ) : (
        <View style={styles.thumbImage} />
      )}
      <Text style={styles.thumbName} numberOfLines={1}>
        {name}
      </Text>
    </Pressable>
  );
}

/** The longest edge a thumbnail asks the host for, at the phone's 3× scale. */
const THUMB_EDGE = 312;

/* A message's runs of calls are gathered afresh on each update, so a strip compares its calls rather than its array:
   while a reply streams, a strip whose calls did not change is not drawn again. */
const sameCalls = (a: ToolPart[], b: ToolPart[]) => a.length === b.length && a.every((part, index) => part === b[index]);

/**
 * A run of tool calls as one flat strip: a sentence saying what the run did, as the core counted
 * it, opening onto a line for each thing it did and, behind that, every call. Only the live run
 * is open until the person opens another.
 */
export const RunStrip = memo(
  function RunStrip({ id, parts, live }: { id: string; parts: ToolPart[]; live: boolean }) {
    const colors = useColors();
    const styles = useStyles(makeStyles);
    const run = runOf(parts);
    const current = parts.find((part) => toolRunning(part.tool));
    const [open, setOpen] = useFold(id, live || current !== undefined);
    const [raw, setRaw] = useFold(`${id}/raw`, false);
    if (run.said.length === 0 && !current) return null;
    const spent = parts.reduce((sum, part) => sum + callTook(part), 0);
    const said = sentence(run, current?.tool.row);
    const lead = run.lines[0] && parts[run.lines[0].calls[0]];
    const clause = run.said[0];
    const icon =
      lead && !callFailed(lead)
        ? lineIcon(lead.tool.row.intent ?? 'use', lead.tool.row.family ?? 'other', lead)
        : clause
          ? lineIcon(clause.intent, clause.family)
          : 'IconCommand';
    const [adds, dels] = parts.reduce(([adds, dels], part) => [adds + (part.diff?.adds ?? 0), dels + (part.diff?.dels ?? 0)], [0, 0]);
    const failed = run.failed ?? 0;
    const total = runCalls(parts);
    return (
      <ImageViewerHost>
        <View style={styles.strip}>
          <Pressable
            onPress={() => setOpen(!open)}
            style={({ pressed }) => [styles.head, pressed && styles.pressed]}
            accessibilityRole="button"
            accessibilityLabel={`${said}${failed ? `, ${failed} failed` : ''}${current ? ', running' : ''}`}
            accessibilityState={{ expanded: open }}>
            <Icon name={icon} size={13} color={clause ? intentTint(clause.intent, colors) : colors.inkDim} />
            {current ? (
              <Shimmer style={styles.said} layout={styles.saidLayout}>
                {said}
              </Shimmer>
            ) : (
              <Text style={[styles.said, styles.saidLayout]} numberOfLines={1}>
                {said}
              </Text>
            )}
            {adds + dels > 0 ? (
              <Text style={styles.headCount}>
                <Text style={{ color: colors.gitAdded }}>+{adds}</Text> <Text style={{ color: colors.gitDeleted }}>−{dels}</Text>
              </Text>
            ) : null}
            {failed > 0 ? <Text style={[styles.headCount, { color: colors.danger }]}>{`${failed} failed`}</Text> : null}
            <View style={styles.spacer} />
            {current ? (
              <LiveSeconds key={current.id} since={current.startedAt} spent={spent} style={styles.headTime} />
            ) : spent > 0 ? (
              <Text style={styles.headTime}>{durationLabel(spent)}</Text>
            ) : null}
            <Chevron open={open} />
          </Pressable>
          {open ? (
            <View style={styles.body}>
              {run.lines.map((line) => (
                <RunLine key={line.calls.join(',')} line={line} run={parts} group={id} />
              ))}
              {linesSayAll(run, parts) ? null : (
                <Pressable onPress={() => setRaw(!raw)} style={styles.rawToggle} hitSlop={6} accessibilityRole="button">
                  <Text style={styles.quietSmall}>{`All ${total} ${total === 1 ? 'call' : 'calls'}`}</Text>
                  <Chevron open={raw} size={9} />
                  {run.hidden ? <Text style={styles.housekeeping}>{`+${run.hidden} housekeeping`}</Text> : null}
                </Pressable>
              )}
              {raw ? (
                <View style={styles.raw}>
                  {parts.map((part, index) => (
                    <ToolRow key={part.id} part={part} last={index === parts.length - 1} group={id} kindWidth={kindWidth(parts)} />
                  ))}
                </View>
              ) : null}
            </View>
          ) : null}
        </View>
      </ImageViewerHost>
    );
  },
  (before, after) => before.id === after.id && before.live === after.live && sameCalls(before.parts, after.parts),
);

/** The widest call name in a run, so the raw rows line their names up in one column. */
const kindWidth = (parts: ToolPart[]) => Math.min(16, Math.max(4, ...parts.map((part) => toolKind(part.tool).length))) * KIND_CHAR + 1;

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    strip: { marginVertical: 4, borderRadius: 10, backgroundColor: translucent(colors.raised, 0.55) },
    head: { flexDirection: 'row', alignItems: 'center', gap: 8, height: 34, paddingHorizontal: 12, borderRadius: 10 },
    pressed: { backgroundColor: colors.active },
    said: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.secondary },
    saidLayout: { flexShrink: 1, minWidth: 0 },
    headCount: { flexShrink: 0, fontFamily: fonts.ui, fontSize: 12.5 },
    headTime: { flexShrink: 0, fontFamily: fonts.ui, fontSize: 12, fontVariant: ['tabular-nums'], color: colors.inkFaint },
    spacer: { flex: 1 },
    turned: { transform: [{ rotate: '90deg' }] },
    body: { paddingHorizontal: 12, paddingBottom: 10 },
    line: { flexDirection: 'row', alignItems: 'center', gap: 10, minHeight: 28 },
    lineIcon: { width: 14, alignItems: 'center' },
    lineMain: { flex: 1, minWidth: 0 },
    lineWords: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.secondary },
    typed: { fontSize: 12, color: colors.secondary },
    name: { flexShrink: 0, fontFamily: fonts.mono, fontSize: 12, color: colors.ink },
    lineNamed: { flexDirection: 'row', alignItems: 'center', gap: 6, overflow: 'hidden' },
    chip: {
      flexShrink: 0,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      height: 22,
      paddingHorizontal: 8,
      borderRadius: 5,
      backgroundColor: translucent(colors.ink, 0.06),
    },
    lineWithin: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 12, color: colors.secondary },
    chipName: { fontFamily: fonts.mono, fontSize: 12, color: colors.ink },
    chipLines: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkDim },
    pictures: { flexDirection: 'row', flexWrap: 'wrap', gap: 10, marginLeft: 24, marginBottom: 4 },
    thumb: { width: 104, gap: 4 },
    thumbImage: { width: 104, height: 70, borderRadius: 5, backgroundColor: colors.sunken },
    thumbName: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkDim },
    lineEnd: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    endText: { fontFamily: fonts.ui, fontSize: 12, fontVariant: ['tabular-nums'], color: colors.inkFaint },
    lineFailure: {
      marginLeft: 24,
      marginBottom: 4,
      paddingHorizontal: 10,
      paddingVertical: 4,
      borderRadius: 5,
      overflow: 'hidden',
      backgroundColor: translucent(colors.danger, 0.08),
      fontFamily: fonts.mono,
      fontSize: 11.5,
      lineHeight: 17,
      color: colors.danger,
    },
    lineDetail: { marginLeft: 24, marginTop: 1, marginBottom: 6 },
    rawToggle: { flexDirection: 'row', alignItems: 'center', gap: 4, alignSelf: 'flex-start', height: 26, marginTop: 4 },
    quiet: { color: colors.inkDim },
    quietSmall: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkDim },
    housekeeping: { marginLeft: 8, fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    raw: { marginTop: 2, marginLeft: 6 },
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
    rowDetail: { flex: 1, marginLeft: 18, marginTop: 2, marginBottom: 6 },
    detailRow: { flexDirection: 'row' },
    detail: { flex: 1 },
    failure: { fontFamily: fonts.mono, fontSize: 11, lineHeight: 17, color: colors.danger },
    well: { borderRadius: 8, backgroundColor: colors.sunken, overflow: 'hidden' },
    command: {
      flexDirection: 'row',
      alignItems: 'flex-start',
      gap: 7,
      paddingLeft: 12,
      paddingRight: 3,
      paddingVertical: 3,
      marginBottom: 4,
    },
    commandText: { flex: 1, paddingVertical: 4, lineHeight: 17, color: colors.ink },
    outputHead: { flexDirection: 'row', alignItems: 'center', gap: 10, height: 26 },
    exit: {
      paddingHorizontal: 7,
      paddingVertical: 1,
      borderRadius: 10,
      overflow: 'hidden',
      backgroundColor: translucent(colors.danger, 0.12),
      fontFamily: fonts.mono,
      fontSize: 11,
      color: mixed(colors.danger, colors.ink, 0.7),
    },
    faint: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    showAll: { flexDirection: 'row', alignItems: 'center', gap: 5, marginLeft: 'auto' },
    whole: { maxHeight: 320 },
    printedPad: { paddingLeft: 12, paddingRight: 30, paddingVertical: 10 },
    printed: { fontSize: 11.5, lineHeight: 19.5, color: colors.secondary },
    copyOutput: { position: 'absolute', top: 4, right: 4 },
    note: { paddingTop: 4, fontFamily: fonts.ui, fontSize: 10.5, color: colors.inkFaint },
    image: { width: '100%', aspectRatio: 4 / 3, marginTop: 4, borderRadius: 8, backgroundColor: colors.sunken },
  });
};
