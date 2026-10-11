import { createContext, useContext, useEffect, useState } from 'react';
import { Animated, Easing, Image, Pressable, StyleSheet, Text, View } from 'react-native';

import { durationLabel } from '@mac/chat/durationLabel';
import { rateLabel, sentLabel, type RowMeta } from '@mac/chat/messageMeta';
import { cutLongText } from '@mac/chat/longText';
import { subagentTask } from '@mac/chat/transcript';
import type { AcpContentBlock, AcpSubagent, AcpTaskNotice, ChatMessage, ChatPart } from '@mac/chat/types';
import { CopyButton } from '@/ui/CopyButton';
import { haptics } from '@/ui/haptics';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Working } from '@/ui/status';
import { fonts, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { Shimmer } from '@/ui/Shimmer';
import { FoldsContext, useFold } from './folds';
import { Markdown } from './Markdown';
import { SentAttachments } from './Attachments';
import type { Attachment, Held, Unsent } from './session';
import { ToolGroup, type ToolPart } from './Tools';

/** The chat's agent, whose mark its subagents carry. */
export const ProviderContext = createContext('agent');

/** How long a press is held before it opens a message's actions. */

/**
 * A message too long to draw at once shows its start and the rest on a tap, unless the person
 * watched it stream in.
 */
function LongText({ id, text, live, style }: { id: string; text: string; live: boolean; style: object }) {
  const styles = useStyles(makeStyles);
  const folds = useContext(FoldsContext);
  const [whole, setWhole] = useFold(`whole/${id}`, false);
  if (live) folds.streamed.add(id);
  const cut = whole || folds.streamed.has(id) ? null : cutLongText(text);
  return (
    <>
      <Markdown text={cut ? cut.head : text} style={style} />
      {cut ? (
        <Pressable onPress={() => setWhole(true)} hitSlop={6} style={styles.showRest} accessibilityRole="button">
          <Text style={styles.showRestText}>Show the rest · {Math.round(cut.hidden / 1000)}k more characters</Text>
        </Pressable>
      ) : null}
    </>
  );
}

const SUBAGENT_WORDS: Record<AcpSubagent['state'], string> = {
  running: 'working',
  completed: 'done',
  failed: 'failed',
  cancelled: 'stopped',
  disconnected: 'lost',
};

function SubagentMark({ state }: { state: AcpSubagent['state'] }) {
  const colors = useColors();
  if (state === 'running') return <Working />;
  if (state === 'completed') return <Icon name="IconCheck" size={13} color={colors.live} />;
  if (state === 'failed') return <Icon name="IconWarning" size={12} color={colors.danger} />;
  if (state === 'disconnected') return <Icon name="IconPlug" size={12} color={colors.danger} />;
  return <Icon name="IconClose" size={11} color={colors.inkFaint} />;
}

/** What a subagent did, kept to one line until it is opened; what it is doing now is over the composer. */
function Subagent({ subagent, untimed }: { subagent: AcpSubagent; untimed: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const provider = useContext(ProviderContext);
  const [open, setOpen] = useFold(`subagent/${subagent.sessionId}`, false);
  const parts = subagent.messages.flatMap((message) => message.parts);
  const calls = parts.filter((part) => part.kind === 'tool').length;
  const running = subagent.state === 'running';
  return (
    <View style={styles.subagent}>
      <Pressable
        onPress={() => setOpen(!open)}
        style={({ pressed }) => [styles.subagentHead, pressed && { backgroundColor: colors.active }]}
        accessibilityRole="button"
        accessibilityLabel={`Subagent ${subagent.name}, ${SUBAGENT_WORDS[subagent.state]}${calls ? `, ${calls} calls` : ''}`}
        accessibilityState={{ expanded: open }}>
        <View style={[styles.chevron, open && { transform: [{ rotate: '90deg' }] }]}>
          <Icon name="IconChevron" size={10} color={colors.inkDim} />
        </View>
        <AgentIcon provider={provider} size={15} />
        <Text style={styles.subagentName} numberOfLines={1}>
          {subagent.name}
        </Text>
        {running ? (
          <Shimmer style={styles.subagentTask} layout={styles.subagentTaskLayout}>
            {subagentTask(subagent.task)}
          </Shimmer>
        ) : (
          <Text style={styles.subagentTask} numberOfLines={1}>
            {subagentTask(subagent.task)}
          </Text>
        )}
        {calls ? (
          <Text style={styles.subagentCalls}>
            {calls} {calls === 1 ? 'call' : 'calls'}
          </Text>
        ) : null}
        <View style={styles.subagentMark}>
          <SubagentMark state={subagent.state} />
        </View>
      </Pressable>
      {open ? (
        <View style={styles.subagentBody}>
          {parts.length ? (
            <Parts id={`subagent/${subagent.sessionId}`} parts={parts} untimed={untimed} live={running} />
          ) : (
            <Text style={styles.empty}>No output yet.</Text>
          )}
        </View>
      ) : null}
    </View>
  );
}

/** A background task that reached its end, and what it said of how it went. */
function Notice({ id, notice }: { id: string; notice: AcpTaskNotice }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [open, setOpen] = useFold(`notice/${id}`, false);
  const summary = notice.summary?.trim();
  return (
    <Pressable
      onPress={summary ? () => setOpen(!open) : undefined}
      disabled={!summary}
      style={styles.notice}
      accessibilityRole={summary ? 'button' : undefined}
      accessibilityLabel={`${notice.name} ${notice.state}${summary ? `. ${summary}` : ''}`}
      accessibilityState={summary ? { expanded: open } : undefined}>
      <View style={styles.noticeHead}>
        <Icon name="IconTimer" size={12} color={notice.state === 'failed' ? colors.danger : colors.inkFaint} />
        <Text style={styles.noticeName} numberOfLines={1}>
          {notice.name}
        </Text>
        <Text style={[styles.noticeState, notice.state === 'failed' && { color: colors.danger }]}>{notice.state}</Text>
      </View>
      {summary ? (
        <Text style={styles.noticeSummary} numberOfLines={open ? undefined : 1} selectable={open}>
          {summary}
        </Text>
      ) : null}
    </Pressable>
  );
}

function userText(message: ChatMessage): string {
  return message.parts.flatMap((part) => (part.kind === 'text' ? [part.text] : [])).join('\n');
}

/** A message's parts in order, with each run of tool calls drawn as one group. */
function Parts({ id, parts, untimed, live }: { id: string; parts: ChatPart[]; untimed: boolean; live: boolean }) {
  const styles = useStyles(makeStyles);
  const runs: (ChatPart | ToolPart[])[] = [];
  for (const part of parts) {
    const previous = runs[runs.length - 1];
    if (part.kind === 'tool') {
      if (Array.isArray(previous)) previous.push(part);
      else runs.push([part]);
    } else runs.push(part);
  }
  return (
    <>
      {runs.map((run, index) => {
        const last = live && index === runs.length - 1;
        if (Array.isArray(run)) return <ToolGroup key={run[0].id} id={`${id}/${run[0].id}`} parts={run} untimed={untimed} live={last} />;
        switch (run.kind) {
          case 'text':
            return run.text.trim() ? <LongText key={run.id} id={run.id} text={run.text} live={last} style={styles.prose} /> : null;
          case 'thought':
            return run.text.trim() ? (
              <View key={run.id} style={styles.thought}>
                <LongText id={run.id} text={run.text.trim()} live={last} style={styles.thoughtText} />
              </View>
            ) : null;
          case 'subagent':
            return <Subagent key={run.id} subagent={run.subagent} untimed={untimed} />;
          case 'notice':
            return <Notice key={run.id} id={run.id} notice={run.notice} />;
          case 'content':
            return <Content key={run.id} block={run.content} />;
          default:
            return <View key={index} />;
        }
      })}
    </>
  );
}

/** An image or file the agent put in its answer. */
function Content({ block }: { block: AcpContentBlock }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  if (block.type === 'text' && block.text?.trim()) return <Markdown text={block.text} style={styles.prose} />;
  if (block.type === 'image' && block.data && block.mimeType?.startsWith('image/')) {
    return (
      <Image
        source={{ uri: `data:${block.mimeType};base64,${block.data}` }}
        style={styles.image}
        resizeMode="contain"
        accessibilityLabel="Image from the agent"
        accessibilityIgnoresInvertColors
      />
    );
  }
  const resource = typeof block.resource === 'object' && block.resource !== null ? (block.resource as { uri?: unknown }) : null;
  const uri = block.uri ?? (typeof resource?.uri === 'string' ? resource.uri : undefined);
  const name = block.title ?? block.name ?? uri?.split('/').pop();
  if (!name) return null;
  return (
    <View style={styles.resource} accessible accessibilityLabel={`File ${name}`}>
      <Icon name={block.mimeType?.startsWith('image/') ? 'IconImage' : 'IconFile'} size={14} color={colors.inkDim} />
      <Text style={styles.resourceText} numberOfLines={1} selectable>
        {name}
      </Text>
    </View>
  );
}

/** What the Mac shows under a message on hover: Copy, when it was sent or finished and how long it took, and how fast it was written. */
function Strip({ meta, mine }: { meta: RowMeta; mine: boolean }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={[styles.strip, mine && styles.stripMine]}>
      <CopyButton value={meta.text} label={mine ? 'message' : 'reply'} size={15} style={styles.stripCopy} />
      {meta.at !== null ? (
        <Text style={styles.stripTime}>
          {meta.took === null ? sentLabel(meta.at) : `${sentLabel(meta.at)} · took ${durationLabel(meta.took)}`}
        </Text>
      ) : null}
      {meta.rate !== null ? <Text style={styles.stripRate}>{rateLabel(meta.rate)}</Text> : null}
    </View>
  );
}

/** A long press is the text's own, for selecting it, so it must not also count as a tap. */
const keepLongPress = () => {};

export function Message({
  message,
  live = false,
  untimed = false,
  unsent,
  onRetry,
  onTap,
  strip,
  sentFiles,
  from = 0,
}: {
  message: ChatMessage;
  live?: boolean;
  untimed?: boolean;
  unsent?: Unsent;
  onRetry?: (messageId: string) => void;
  /** A tap, which shows or hides the strip under the message it belongs to. */
  onTap?: (message: ChatMessage) => void;
  /** The strip under this message, while it is shown. */
  strip?: RowMeta | null;
  sentFiles: ReadonlyMap<string, Attachment>;
  /** Where the shown parts start, when the work before them is folded away. */
  from?: number;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const tap = onTap
    ? () => {
        haptics.select();
        onTap(message);
      }
    : undefined;
  if (message.role === 'user') {
    const failed = unsent?.state === 'failed';
    const text = userText(message);
    return (
      <View style={styles.userRow}>
        {message.attachments?.length ? <SentAttachments paths={message.attachments} sentFiles={sentFiles} /> : null}
        {text || !message.attachments?.length ? (
          <Pressable
            onPress={tap}
            onLongPress={keepLongPress}
            style={[styles.bubble, unsent?.state === 'sending' && { opacity: 0.7 }, failed && styles.failedBubble]}
            accessibilityHint={tap ? 'Tap for copy and the time it was sent' : undefined}>
            <Text style={styles.userText} selectable>
              {text}
            </Text>
          </Pressable>
        ) : null}
        {failed ? (
          <Pressable
            onPress={() => onRetry?.(message.id)}
            hitSlop={10}
            style={styles.unsent}
            accessibilityRole="button"
            accessibilityLabel={`Not sent${unsent?.problem ? `: ${unsent.problem}` : ''}. Retry`}>
            <Icon name="IconWarning" size={12} color={colors.danger} />
            <Text style={[styles.unsentText, { color: colors.danger }]}>Not sent</Text>
            <Text style={styles.unsentText}>·</Text>
            <Text style={[styles.unsentText, { color: colors.ink }]}>Retry</Text>
          </Pressable>
        ) : unsent?.state === 'sending' ? (
          <Text style={styles.queuedLabel}>Sending…</Text>
        ) : strip ? (
          <Strip meta={strip} mine />
        ) : null}
      </View>
    );
  }
  return (
    <Pressable onPress={tap} onLongPress={keepLongPress} accessibilityHint={tap ? 'Tap for copy and how long it took' : undefined}>
      <Parts id={message.id} parts={from > 0 ? message.parts.slice(from) : message.parts} untimed={untimed} live={live} />
      {strip ? <Strip meta={strip} mine={false} /> : null}
    </Pressable>
  );
}

/** A finished answer's work, folded above the reply it closed with, as on the Mac. */
export function WorkSummary({ took, calls, open, onToggle }: { took: number | null; calls: number; open: boolean; onToggle: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const worked = took === null ? 'Worked' : `Worked for ${durationLabel(took)}`;
  const counted = `${calls} tool call${calls === 1 ? '' : 's'}`;
  return (
    <View style={styles.work}>
      <Pressable
        onPress={() => {
          haptics.select();
          onToggle();
        }}
        style={styles.workSum}
        hitSlop={6}
        accessibilityRole="button"
        accessibilityLabel={calls > 0 ? `${worked}, ${counted}` : worked}
        accessibilityState={{ expanded: open }}>
        <Text style={styles.workText}>{worked}</Text>
        {calls > 0 ? (
          <>
            <Text style={[styles.workCalls, styles.workDot]}>·</Text>
            <Text style={styles.workCalls}>{counted}</Text>
          </>
        ) : null}
        <View style={[styles.workChevron, open && { transform: [{ rotate: '90deg' }] }]}>
          <Icon name="IconChevron" size={11} color={colors.inkDim} />
        </View>
      </Pressable>
    </View>
  );
}

/** Messages waiting behind the running turn, faded where they will go. */
export function Queued({ held, sentFiles }: { held: readonly Held[]; sentFiles: ReadonlyMap<string, Attachment> }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.userRow}>
      {held.map((message) => {
        const paths = message.attachments.flatMap((attachment) => (attachment.path ? [attachment.path] : []));
        return (
          <View key={message.id} style={styles.queued}>
            {paths.length ? <SentAttachments paths={paths} sentFiles={sentFiles} /> : null}
            {message.text ? (
              <View style={styles.bubble}>
                <Text style={styles.userText}>{message.text}</Text>
              </View>
            ) : null}
          </View>
        );
      })}
      <Text style={styles.queuedLabel}>Sends when this turn ends</Text>
    </View>
  );
}

/** Above the first message while the host has turns from before it, which arrive as it comes into view. */
export function Earlier({ failed, onRetry }: { failed: boolean; onRetry: () => void }) {
  const styles = useStyles(makeStyles);
  if (failed) {
    return (
      <Pressable
        onPress={onRetry}
        style={styles.earlier}
        hitSlop={8}
        accessibilityRole="button"
        accessibilityLabel="Couldn't load earlier messages. Retry">
        <Text style={styles.earlierText}>{"Couldn't load earlier messages ·"}</Text>
        <Text style={[styles.earlierText, styles.retry]}>Retry</Text>
      </Pressable>
    );
  }
  return (
    <View style={styles.earlier}>
      <Working />
      <Text style={styles.earlierText}>Loading earlier messages…</Text>
    </View>
  );
}

/** The working line: the agent's logo breathing beside what it is doing, and for how long. */
export function Activity({ provider, label, since }: { provider: string; label: string; since: number }) {
  const styles = useStyles(makeStyles);
  const [breath] = useState(() => new Animated.Value(1));
  const [seconds, setSeconds] = useState(() => Math.round((Date.now() - since) / 1000));
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
    <View style={styles.activity} accessible accessibilityLabel={label}>
      <Animated.View style={{ opacity: breath }}>
        <AgentIcon provider={provider} size={20} />
      </Animated.View>
      <Shimmer style={styles.activityText} layout={styles.activityLabel}>
        {label}
      </Shimmer>
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
    userText: { fontFamily: fonts.ui, fontSize: 14, lineHeight: 22, color: colors.ink },
    failedBubble: { borderColor: colors.danger },
    unsent: { flexDirection: 'row', alignItems: 'center', gap: 5, marginTop: 5 },
    unsentText: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.inkFaint },
    image: { width: '100%', aspectRatio: 4 / 3, borderRadius: 8, marginVertical: 6, backgroundColor: colors.sunken },
    resource: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      alignSelf: 'flex-start',
      maxWidth: '100%',
      marginVertical: 6,
      paddingVertical: 7,
      paddingHorizontal: 10,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    resourceText: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 12, color: colors.ink },
    retry: { color: colors.ink, fontFamily: fonts.uiMedium },
    queued: { alignItems: 'flex-end', alignSelf: 'stretch', opacity: 0.55, marginBottom: 6 },
    queuedLabel: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkFaint, marginTop: 4 },
    work: {
      marginTop: 8,
      marginBottom: 4,
      paddingBottom: 4,
      borderBottomWidth: StyleSheet.hairlineWidth,
      borderBottomColor: colors.border,
    },
    workSum: { flexDirection: 'row', alignItems: 'center', gap: 8, minHeight: 24, alignSelf: 'flex-start' },
    workText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    workCalls: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkFaint },
    workDot: { marginHorizontal: -2 },
    workChevron: { opacity: 0.7 },
    strip: { flexDirection: 'row', alignItems: 'center', gap: 8, height: 24, marginTop: 6 },
    stripMine: { justifyContent: 'flex-end' },
    stripCopy: { width: 24, height: 24 },
    stripTime: { fontFamily: fonts.ui, fontSize: 11, fontVariant: ['tabular-nums'], color: colors.inkDim },
    stripRate: { marginLeft: 'auto', fontFamily: fonts.mono, fontSize: 11, fontStyle: 'italic', color: colors.inkDim },
    prose: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 23, color: colors.ink },
    thought: { marginVertical: 8 },
    thoughtText: { fontFamily: fonts.uiItalic, fontSize: 12.5, lineHeight: 19.5, color: colors.inkFaint },
    showRest: { alignSelf: 'flex-start', marginTop: 6, marginBottom: 4 },
    showRestText: { fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.tertiary },
    subagent: { marginVertical: 6 },
    subagentHead: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      minHeight: 36,
      paddingLeft: 10,
      paddingRight: 11,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: translucent(colors.raised, 0.6),
    },
    chevron: { opacity: 0.7 },
    subagentName: { flexShrink: 0, maxWidth: '45%', fontFamily: fonts.mono, fontSize: 11.5, color: colors.ink },
    subagentTask: { flex: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    subagentTaskLayout: { flex: 1 },
    subagentCalls: { fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint },
    subagentMark: { width: 14, alignItems: 'center' },
    subagentBody: { paddingLeft: 14, paddingTop: 4 },
    empty: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkFaint, paddingVertical: 6 },
    notice: {
      alignSelf: 'flex-start',
      maxWidth: '100%',
      gap: 3,
      marginVertical: 5,
      paddingVertical: 6,
      paddingHorizontal: 10,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
    },
    noticeHead: { flexDirection: 'row', alignItems: 'center', gap: 7 },
    noticeName: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 11.5, color: colors.ink },
    noticeState: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    noticeSummary: { fontFamily: fonts.ui, fontSize: 12, lineHeight: 17, color: colors.inkFaint },
    earlier: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 8, paddingTop: 10, paddingBottom: 6 },
    earlierText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    activity: { flexDirection: 'row', alignItems: 'center', gap: 9, marginTop: 14, marginBottom: 6 },
    activityText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkFaint },
    activityLabel: { flexShrink: 1 },
    activityTime: { fontFamily: fonts.mono, fontSize: 10, color: colors.inkFaint },
  });
};
