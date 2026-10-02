import { useEffect, useState } from 'react';
import { Animated, Easing, Pressable, StyleSheet, Text, View } from 'react-native';

import { durationLabel } from '@mac/chat/durationLabel';
import { toolKind, toolRunning, toolTarget } from '@mac/chat/toolLabels';
import type { ChatMessage, ChatPart } from '@mac/chat/types';
import { AgentIcon, Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { Markdown } from './Markdown';

type ToolPart = Extract<ChatPart, { kind: 'tool' }>;

/** The verb, icon and colour of each kind of call, as the Mac's tool rows draw them. */
function look(kind: string, colors: Palette): { icon: IconName; color: string } {
  switch (kind) {
    case 'run':
      return { icon: 'IconCommand', color: colors.toolRun };
    case 'read':
      return { icon: 'IconFile', color: colors.toolRead };
    case 'search':
      return { icon: 'IconSearch', color: colors.toolRead };
    case 'fetch':
      return { icon: 'IconGlobe', color: colors.toolRead };
    case 'edit':
    case 'move':
      return { icon: 'IconPencil', color: colors.toolEdit };
    case 'delete':
      return { icon: 'IconPencil', color: colors.toolDelete };
    default:
      return { icon: 'IconAgent', color: colors.inkDim };
  }
}

function ToolRow({ part, last, untimed }: { part: ToolPart; last: boolean; untimed: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const kind = toolKind(part.tool);
  const { icon, color } = look(kind, colors);
  const failed = part.tool.status === 'failed';
  const running = toolRunning(part.tool);
  const spent =
    !untimed && part.startedAt && part.endedAt && part.endedAt > part.startedAt ? durationLabel(part.endedAt - part.startedAt) : null;
  return (
    <View style={styles.tool}>
      <View style={last ? styles.elbow : styles.spine} />
      <View style={[styles.tick, last && { backgroundColor: 'transparent' }]} />
      <Icon name={failed ? 'IconWarning' : icon} size={12} color={failed ? colors.danger : color} />
      <Text style={[styles.kind, { color: failed ? colors.danger : color }]}>{kind}</Text>
      <Text style={[styles.target, running && { color: colors.inkDim }]} numberOfLines={1}>
        {toolTarget(part.tool)}
      </Text>
      <View style={styles.toolEnd}>
        {part.diff ? (
          <Text style={styles.endText}>
            <Text style={{ color: colors.gitAdded }}>+{part.diff.adds}</Text>{' '}
            <Text style={{ color: colors.gitDeleted }}>−{part.diff.dels}</Text>
          </Text>
        ) : null}
        {!part.diff && spent ? <Text style={styles.endText}>{spent}</Text> : null}
      </View>
    </View>
  );
}

function ToolGroup({ parts, untimed }: { parts: ToolPart[]; untimed: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const working = parts.some((part) => toolRunning(part.tool));
  const [open, setOpen] = useState(working);
  const [wasWorking, setWasWorking] = useState(working);
  if (working !== wasWorking) {
    setWasWorking(working);
    if (working) setOpen(true);
  }
  const started = parts[0]?.startedAt;
  const ended = parts[parts.length - 1]?.endedAt;
  return (
    <View style={styles.tools}>
      <Pressable
        onPress={() => setOpen(!open)}
        style={styles.summary}
        hitSlop={6}
        accessibilityRole="button"
        accessibilityState={{ expanded: open }}>
        <Text style={styles.summaryText}>
          {parts.length} tool call{parts.length === 1 ? '' : 's'}
        </Text>
        {!untimed && started && ended && ended > started ? <Text style={styles.summaryTime}>{durationLabel(ended - started)}</Text> : null}
        <View style={[styles.summaryChevron, open && { transform: [{ rotate: '90deg' }] }]}>
          <Icon name="IconChevron" size={11} color={colors.inkDim} />
        </View>
      </Pressable>
      {open ? (
        <View style={styles.toolsBody}>
          {parts.map((part, index) => (
            <ToolRow key={part.id} part={part} last={index === parts.length - 1} untimed={untimed} />
          ))}
        </View>
      ) : null}
    </View>
  );
}

function userText(message: ChatMessage): string {
  return message.parts.flatMap((part) => (part.kind === 'text' ? [part.text] : [])).join('\n');
}

function Assistant({ message, untimed }: { message: ChatMessage; untimed: boolean }) {
  const styles = useStyles(makeStyles);
  const runs: (ChatPart | ToolPart[])[] = [];
  for (const part of message.parts) {
    const previous = runs[runs.length - 1];
    if (part.kind === 'tool') {
      if (Array.isArray(previous)) previous.push(part);
      else runs.push([part]);
    } else runs.push(part);
  }
  return (
    <>
      {runs.map((run, index) => {
        if (Array.isArray(run)) return <ToolGroup key={run[0].id} parts={run} untimed={untimed} />;
        switch (run.kind) {
          case 'text':
            return run.text.trim() ? <Markdown key={run.id} text={run.text} style={styles.prose} /> : null;
          case 'thought':
            return run.text.trim() ? (
              <Text key={run.id} style={styles.thought}>
                {run.text.trim()}
              </Text>
            ) : null;
          case 'subagent':
            return (
              <Text key={run.id} style={styles.note}>
                {run.subagent.name} · {run.subagent.state}
              </Text>
            );
          case 'notice':
            return (
              <Text key={run.id} style={styles.note}>
                {run.notice.name} {run.notice.state}
              </Text>
            );
          default:
            return <View key={index} />;
        }
      })}
    </>
  );
}

export function Message({ message, untimed = false }: { message: ChatMessage; untimed?: boolean }) {
  const styles = useStyles(makeStyles);
  if (message.role === 'user') {
    return (
      <View style={styles.userRow}>
        <View style={styles.bubble}>
          <Markdown text={userText(message)} style={styles.userText} />
        </View>
      </View>
    );
  }
  return <Assistant message={message} untimed={untimed} />;
}

export function Queued({ text }: { text: string }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.userRow}>
      <View style={[styles.bubble, { opacity: 0.55 }]}>
        <Text style={styles.userText}>{text}</Text>
      </View>
      <Text style={styles.queuedLabel}>Sends when this turn ends</Text>
    </View>
  );
}

/** The working line: the agent's logo breathing beside what it is doing, and for how long. */
export function Activity({ provider, label, since }: { provider: string; label: string; since: number }) {
  const styles = useStyles(makeStyles);
  const [breath] = useState(() => new Animated.Value(1));
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    const loop = Animated.loop(
      Animated.sequence([
        Animated.timing(breath, { toValue: 0.5, duration: 750, easing: Easing.inOut(Easing.ease), useNativeDriver: true }),
        Animated.timing(breath, { toValue: 1, duration: 750, easing: Easing.inOut(Easing.ease), useNativeDriver: true }),
      ]),
    );
    loop.start();
    const tick = setInterval(() => setSeconds(Math.round((Date.now() - since) / 1000)), 1000);
    return () => {
      loop.stop();
      clearInterval(tick);
    };
  }, [breath, since]);
  return (
    <View style={styles.activity}>
      <Animated.View style={{ opacity: breath }}>
        <AgentIcon provider={provider} size={20} />
      </Animated.View>
      <Text style={styles.activityText}>{label}</Text>
      {seconds > 0 ? <Text style={styles.activityTime}>{durationLabel(seconds * 1000)}</Text> : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    userRow: { alignItems: 'flex-end', marginTop: 14, marginBottom: 6 },
    bubble: {
      maxWidth: '84%',
      paddingVertical: 9,
      paddingHorizontal: 13,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    userText: { fontFamily: fonts.ui, fontSize: 15, lineHeight: 24, color: colors.ink },
    queuedLabel: { fontFamily: fonts.ui, fontSize: 11.5, color: colors.inkFaint, marginTop: 4 },
    prose: { fontFamily: fonts.ui, fontSize: 15.5, lineHeight: 25, color: colors.ink },
    thought: { fontFamily: fonts.uiItalic, fontSize: 13.5, lineHeight: 21, color: colors.inkFaint, marginVertical: 8 },
    note: { fontFamily: fonts.ui, fontSize: 13, color: colors.inkDim, marginVertical: 6 },
    tools: { marginVertical: 6 },
    summary: { flexDirection: 'row', alignItems: 'center', gap: 8, minHeight: 26, alignSelf: 'flex-start' },
    summaryText: { fontFamily: fonts.ui, fontSize: 13, color: colors.inkDim },
    summaryTime: { fontFamily: fonts.mono, fontSize: 11.5, color: colors.inkDim },
    summaryChevron: { opacity: 0.7 },
    toolsBody: { marginLeft: 6, paddingVertical: 2 },
    tool: { flexDirection: 'row', alignItems: 'center', minHeight: 26, gap: 8 },
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
    kind: { fontFamily: fonts.mono, fontSize: 12 },
    target: { flex: 1, fontFamily: fonts.mono, fontSize: 12, color: colors.ink },
    toolEnd: { flexDirection: 'row', gap: 6 },
    endText: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkDim },
    activity: { flexDirection: 'row', alignItems: 'center', gap: 9, marginTop: 14, marginBottom: 6 },
    activityText: { fontFamily: fonts.ui, fontSize: 13, color: colors.inkFaint },
    activityTime: { fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint },
  });
};
