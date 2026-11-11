import { Fragment, useContext, useMemo, useState, type ReactNode } from 'react';
import { Animated, Easing, Image, Pressable, StyleSheet, Text, View, type LayoutChangeEvent } from 'react-native';

import { durationLabel } from '@shared/chat/durationLabel';
import { basename } from '@shared/lib/paths';
import { clockLabel, rateLabel, sentLabel } from '@shared/chat/messageMeta';
import type { MdElement } from '@shared/markdown/types';
import {
  attachmentName,
  groupParts,
  shownParts,
  SUBAGENT_MARKS,
  SUBAGENT_WORDS,
  subagentCost,
  subagentTask,
  type SubagentMark,
} from '@shared/chat/transcript';
import { teamCount, teamTook, teamWords } from '@shared/chat/team';
import { callsLabel, workedLabel } from '@shared/chat/toolView';
import type { ChatMessage, ChatPart, ContentBlock, SubagentCard, TaskNotice as Notice, TeamLine } from '@shared/wire.generated';
import { CopyButton } from '@/ui/CopyButton';
import { haptics } from '@/ui/haptics';
import { AgentFace } from '@/devices/Buddy';
import { AgentIcon, Icon } from '@/ui/Icon';
import { fonts, mixed, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { useStill } from '@/ui/motion';
import { Shimmer } from '@/ui/Shimmer';
import { useFold } from './folds';
import { FindingContext, markTyped, type Finding } from './found';
import { Markdown, plainBlocks } from './Markdown';
import { SentAttachments } from './Attachments';
import { hostImagePath } from './FileRef';
import { useHostFile } from './hostFiles';
import { ImageViewer } from './ImageViewer';
import type { Strip as StripMeta } from './messageMeta';
import type { Unsent } from './outbox';
import type { Attachment } from './uploads';
import { ToolGroup } from './Tools';
import { AnsweredQuestions } from './Question';
import { Bubble, From, makeBubbleStyles } from './Bubble';
import { AgentContext, EarlierThoughtsContext, ProviderContext, SubagentsContext } from '@/chat/transcriptContext';
import { AccountLine, AgentNoticeLine, PageCard, TaskNotice } from '@/chat/Notices';
import { ContextChips } from '@/chat/ContextChips';
import { Editing, MessageEditor } from '@/chat/MessageEditor';
import { FiredTrigger } from '@/chat/FiredTrigger';
import { LongText } from '@/chat/LongText';

/** A subagent's mark and word once it is not doing anything: done, failed, lost or stopped. */
function SubagentEnd({ state }: { state: SubagentCard['state'] }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const mark: SubagentMark = SUBAGENT_MARKS[state];
  const tint =
    mark === 'check'
      ? mixed(colors.live, colors.inkFaint, 0.58)
      : mark === 'warning' || mark === 'plug'
        ? colors.danger
        : mark === 'spinner'
          ? mixed(colors.accent, colors.inkFaint, 0.62)
          : colors.inkFaint;
  const icon = ({ spinner: 'IconHourglass', check: 'IconCheck', warning: 'IconWarning', plug: 'IconPlug', close: 'IconClose' } as const)[
    mark
  ];
  const loud = mark === 'warning' || mark === 'plug';
  return (
    <View style={styles.subagentEnd}>
      <Icon name={icon} size={12} color={tint} />
      <Text style={[styles.subagentWord, loud && { color: tint }]}>{SUBAGENT_WORDS[state]}</Text>
    </View>
  );
}

/** A subagent as a strip: who, its task, what it is doing now or how it ended, and what it cost; it opens onto its work. */
function Subagent({ subagent }: { subagent: SubagentCard }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const provider = useContext(ProviderContext);
  const rows = useContext(SubagentsContext).rows[subagent.sessionId];
  const [open, setOpen] = useFold(`subagent/${subagent.sessionId}`, false);
  const parts = useMemo(() => (rows ?? []).flatMap((message) => message.parts), [rows]);
  const cost = subagentCost(subagent);
  const running = subagent.state === 'running';
  const row = subagent.lastTool?.row;
  const doing = running ? (row?.activity ?? row?.subject ?? row?.target) : undefined;
  return (
    <View style={styles.subagent}>
      <Pressable
        onPress={() => setOpen(!open)}
        style={({ pressed }) => [styles.subagentHead, pressed && styles.subagentPressed]}
        accessibilityRole="button"
        accessibilityLabel={`Subagent ${subagent.name}, ${doing ?? SUBAGENT_WORDS[subagent.state]}${cost ? `, ${cost}` : ''}`}
        accessibilityState={{ expanded: open }}>
        <AgentIcon provider={provider} size={16} />
        <Text style={styles.subagentName} numberOfLines={1}>
          {subagent.name}
        </Text>
        <Text style={styles.subagentTask} numberOfLines={1}>
          {subagentTask(subagent.task)}
        </Text>
        {doing ? (
          <Shimmer style={styles.subagentDoing} layout={styles.subagentDoingLayout}>
            {doing}
          </Shimmer>
        ) : (
          <SubagentEnd state={subagent.state} />
        )}
        <View style={styles.subagentSpacer} />
        {cost ? (
          <Text style={styles.subagentCalls} numberOfLines={1}>
            {cost}
          </Text>
        ) : null}
        <View style={open && styles.turned}>
          <Icon name="IconChevron" size={11} color={colors.inkDim} />
        </View>
      </Pressable>
      {open ? (
        <View style={styles.subagentBody}>
          {parts.length ? (
            <Parts id={`subagent/${subagent.sessionId}`} parts={parts} live={running} />
          ) : (
            <Text style={styles.empty}>No output yet.</Text>
          )}
        </View>
      ) : null}
    </View>
  );
}

/** A background task that reached its end, and what it said of how it went. */
function TaskNoticeOrCard({ id, notice }: { id: string; notice: Notice }) {
  const subagent = useContext(SubagentsContext).card(notice.subagent ?? '');
  if (subagent) return <Subagent subagent={subagent} />;
  return <TaskNotice id={id} notice={notice} />;
}

/** The line a streaming turn's older thoughts fold behind, which opens them in place. */
function EarlierThinking({ count, open, onToggle }: { count: number; open: boolean; onToggle: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={onToggle}
      style={styles.earlier}
      hitSlop={6}
      accessibilityRole="button"
      accessibilityLabel={`Earlier thinking, ${count} ${count === 1 ? 'thought' : 'thoughts'}`}
      accessibilityState={{ expanded: open }}>
      <View style={open && styles.turned}>
        <Icon name="IconChevron" size={11} color={colors.inkDim} />
      </View>
      <Text style={styles.earlierText}>Earlier thinking</Text>
      <Text style={styles.earlierCount}>{`${count} ${count === 1 ? 'thought' : 'thoughts'}`}</Text>
    </Pressable>
  );
}

function Parts({ id, parts, live }: { id: string; parts: ChatPart[]; live: boolean }) {
  const styles = useStyles(makeStyles);
  const groups = groupParts(parts);
  const earlier = useContext(EarlierThoughtsContext);
  const [earlierOpen, setEarlierOpen] = useFold(earlier.key, false);
  return (
    <>
      {groups.map((group, index) => {
        const last = live && index === groups.length - 1;
        if ('tools' in group) return <ToolGroup key={group.id} id={`${id}/${group.id}`} parts={group.tools} live={last} />;
        const run = group.part;
        switch (run.kind) {
          case 'text':
            return run.text.trim() ? (
              <View key={run.id} style={index > 0 && styles.said}>
                <LongText id={run.id} text={run.text} md={run.md} live={last} style={styles.prose} />
              </View>
            ) : null;
          case 'thought': {
            const place = earlier.ids.indexOf(run.id);
            const line =
              place === 0 ? (
                <EarlierThinking
                  key={`${run.id}/earlier`}
                  count={earlier.ids.length}
                  open={earlierOpen}
                  onToggle={() => setEarlierOpen(!earlierOpen)}
                />
              ) : null;
            if (place >= 0 && !earlierOpen) return line;
            const thought = run.text.trim() ? (
              <View key={run.id} style={styles.thought}>
                <LongText id={run.id} text={run.text.trim()} md={run.md} live={last} style={styles.thoughtText} />
              </View>
            ) : null;
            return line ? (
              <Fragment key={run.id}>
                {line}
                {thought}
              </Fragment>
            ) : (
              thought
            );
          }
          case 'subagent':
            return <Subagent key={run.id} subagent={run.subagent} />;
          case 'notice':
            return <TaskNoticeOrCard key={run.id} id={run.id} notice={run.notice} />;
          case 'content':
            return <Content key={run.id} block={run.content} />;
          case 'agent_notice':
            return <AgentNoticeLine key={run.id} notice={run.notice} />;
          case 'account':
            return <AccountLine key={run.id} move={run.move} />;
          case 'tool':
            return run.page ? <PageCard key={run.id} page={run.page} /> : null;
          default:
            return <View key={index} />;
        }
      })}
    </>
  );
}

function Picture({ uri, name }: { uri: string; name: string }) {
  const styles = useStyles(makeStyles);
  const [open, setOpen] = useState(false);
  return (
    <>
      <Pressable onPress={() => setOpen(true)} accessibilityRole="imagebutton" accessibilityLabel={`${name}, open`}>
        <Image source={{ uri }} style={styles.image} resizeMode="contain" accessibilityIgnoresInvertColors />
      </Pressable>
      {open ? <ImageViewer uri={uri} name={name} onClose={() => setOpen(false)} /> : null}
    </>
  );
}

function ResourceChip({ name, image }: { name: string; image: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.resource} accessible accessibilityLabel={`File ${name}`}>
      <Icon name={image ? 'IconImage' : 'IconFile'} size={14} color={colors.inkDim} />
      <Text style={styles.resourceText} numberOfLines={1} selectable>
        {name}
      </Text>
    </View>
  );
}

/** A picture the answer links to on the host, previewed as the Mac previews it, or its file until it arrives. */
function HostPicture({ path, name }: { path: string; name: string }) {
  const uri = useHostFile(path, true);
  return uri ? <Picture uri={uri} name={name} /> : <ResourceChip name={name} image />;
}

function Content({ block }: { block: ContentBlock }) {
  const styles = useStyles(makeStyles);
  if (block.type === 'text' && block.text.trim()) return <Markdown blocks={plainBlocks(block.text)} style={styles.prose} />;
  if (block.type === 'image' && block.data && block.mimeType.startsWith('image/')) {
    return <Picture uri={`data:${block.mimeType};base64,${block.data}`} name={attachmentName(block.mimeType)} />;
  }
  const uri =
    block.type === 'image' || block.type === 'resource_link' ? block.uri : block.type === 'resource' ? block.resource.uri : undefined;
  const titled = block.type === 'resource_link' ? (block.title ?? block.name) : undefined;
  const name = titled ?? (uri ? basename(uri) : undefined);
  if (!name) return null;
  const image = uri ? hostImagePath(uri) : null;
  if (image) return <HostPicture path={image} name={name} />;
  const mimeType = block.type === 'image' || block.type === 'audio' || block.type === 'resource_link' ? block.mimeType : undefined;
  return <ResourceChip name={name} image={Boolean(mimeType?.startsWith('image/'))} />;
}

/** What the Mac shows under a message on hover: Copy, Edit on a message that can be written again, when it was sent or finished and how long it took, and how fast it was written. */
function Strip({ meta, mine, onEdit }: { meta: StripMeta; mine: boolean; onEdit?: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={[styles.strip, mine && styles.stripMine]}>
      <CopyButton value={meta.text} label={mine ? 'message' : 'reply'} size={15} style={styles.stripCopy} />
      {onEdit ? (
        <Pressable
          onPress={onEdit}
          hitSlop={10}
          style={({ pressed }) => [styles.stripCopy, styles.stripButton, pressed && { backgroundColor: colors.active }]}
          accessibilityRole="button"
          accessibilityLabel="Edit message">
          <Icon name="IconPencil" size={15} color={colors.inkFaint} />
        </Pressable>
      ) : null}
      {meta.at !== undefined ? (
        <Text style={styles.stripTime}>
          {meta.took === undefined ? sentLabel(meta.at) : `${sentLabel(meta.at)} · took ${durationLabel(meta.took)}`}
        </Text>
      ) : null}
      {meta.rate !== undefined ? <Text style={styles.stripRate}>{rateLabel(meta.rate)}</Text> : null}
    </View>
  );
}

/** A sent message's words with the places the find bar found in them marked. */
function TypedFound({ parts, finding }: { parts: Extract<ChatPart, { kind: 'text' }>[]; finding: Finding }) {
  const styles = useStyles(makeStyles);
  return parts.map((part, index) => {
    const current = finding.current?.part === part.id ? finding.current.range : null;
    const runs = markTyped(part.text, part.md?.blocks as MdElement[] | undefined, finding.byPart.get(part.id), current);
    return (
      <Fragment key={part.id}>
        {index > 0 ? '\n\n' : null}
        {runs.map((run, at) =>
          run.found ? (
            <Text key={at} style={run.current ? styles.foundCurrent : styles.found}>
              {run.text}
            </Text>
          ) : (
            run.text
          ),
        )}
      </Fragment>
    );
  });
}

/** A message the person sent themselves, as opposed to letters or a trigger's prompt. */
export function plainlySent(message: ChatMessage | undefined): boolean {
  return message?.role === 'user' && !message.letters?.length;
}

type Letter = NonNullable<ChatMessage['letters']>[number];

/** A message's letters, with each run of answers to one agent's questions together as one thread. */
function letterGroups(letters: readonly Letter[]) {
  const groups: ({ kind: 'answers'; at: number; letters: Letter[] } | { kind: 'letter'; at: number; letter: Letter })[] = [];
  letters.forEach((letter, at) => {
    const last = groups[groups.length - 1];
    if (letter.kind !== 'answer') groups.push({ kind: 'letter', at, letter });
    else if (last?.kind === 'answers' && last.letters[0].agentId === letter.agentId) last.letters.push(letter);
    else groups.push({ kind: 'answers', at, letters: [letter] });
  });
  return groups;
}

/** A long press is the text's own, for selecting it, so it must not also count as a tap. */
const keepLongPress = () => {};

export function Message({
  message,
  live = false,
  unsent,
  onRetry,
  onTap,
  strip,
  sentFiles,
  kept = null,
  onEdit,
  editing,
  followsOwn = false,
  followedByOwn = false,
}: {
  message: ChatMessage;
  live?: boolean;
  unsent?: Unsent;
  onRetry?: (messageId: string) => void;
  /** A tap, which shows or hides the strip under the message it belongs to. */
  onTap?: (message: ChatMessage) => void;
  /** The strip under this message, while it is shown. */
  strip?: StripMeta | null;
  sentFiles: ReadonlyMap<string, Attachment>;
  /** Where the shown parts start, when the work before them is folded away. */
  kept?: readonly number[] | null;
  /** Writes this message again; absent when the chat cannot take it back. */
  onEdit?: (message: ChatMessage) => void;
  /** This message is being written again. */
  editing?: Editing;
  /** The message before this one was also the person's own, so this one stacks under it. */
  followsOwn?: boolean;
  /** The next message is the person's own too, so it stacks under this one. */
  followedByOwn?: boolean;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const bubbleText = useStyles(makeBubbleStyles);
  const chatAgent = useContext(AgentContext);
  const finding = useContext(FindingContext);
  const tap = onTap
    ? () => {
        haptics.select();
        onTap(message);
      }
    : undefined;
  const letters = message.letters;
  if (letters?.length) {
    return (
      <Pressable
        onPress={tap}
        onLongPress={keepLongPress}
        style={styles.letters}
        accessibilityHint={tap ? 'Tap for copy and the time it came' : undefined}>
        {letterGroups(letters).map((group) =>
          group.kind === 'answers' ? (
            <AnsweredQuestions
              key={group.at}
              agentId={group.letters[0].agentId ?? ''}
              cli={group.letters[0].cli}
              asker={group.letters[0].agentId === chatAgent ? null : group.letters[0].name}
              answers={group.letters.map((letter) => ({
                question: letter.question ?? '',
                answer: letter.text,
                secret: letter.secret ?? false,
                files: letter.attachments?.length ? <SentAttachments paths={letter.attachments} sentFiles={sentFiles} /> : null,
              }))}
            />
          ) : group.letter.trigger ? (
            <FiredTrigger key={group.at} id={`${message.id}/letter/${group.at}`} fired={group.letter.trigger} at={message.sentAt} />
          ) : (
            <From
              key={group.at}
              at={message.sentAt}
              who={
                <>
                  <AgentFace agentId={group.letter.agentId} provider={group.letter.cli} size={17} />
                  <Text style={styles.letterName} numberOfLines={1}>
                    @{group.letter.name}
                  </Text>
                </>
              }>
              <LongText
                id={`${message.id}/letter/${group.at}`}
                text={group.letter.text}
                md={group.letter.tree}
                live={false}
                style={bubbleText.text}
              />
            </From>
          ),
        )}
        {strip ? <Strip meta={{ ...strip, text: letters.map((letter) => letter.text).join('\n\n') }} mine={false} /> : null}
      </Pressable>
    );
  }
  if (message.role === 'user') {
    const failed = unsent?.state === 'failed';
    const typed = message.parts.flatMap((part) => (part.kind === 'text' ? [part] : []));
    const text = typed.map((part) => part.text).join('\n\n');
    const spans = message.spans ?? [];
    const context = (message.context ?? []).filter((item) => !spans.some((span) => span.chip?.carried === item.uri));
    return (
      <View style={[styles.userRow, followsOwn && styles.followsOwn, followedByOwn && styles.followedByOwn]}>
        {message.attachments?.length ? <SentAttachments paths={message.attachments} sentFiles={sentFiles} /> : null}
        {context.length ? <ContextChips items={context} /> : null}
        {editing ? (
          <MessageEditor key={message.id} text={text} editing={editing} />
        ) : text || (!message.attachments?.length && !context.length) ? (
          <Pressable
            onPress={tap}
            onLongPress={keepLongPress}
            style={[styles.bubble, unsent?.state === 'sending' && { opacity: 0.7 }]}
            accessibilityHint={tap ? 'Tap for copy and the time it was sent' : undefined}>
            <Bubble style={failed && styles.failedBubble}>
              <Text style={bubbleText.text} selectable>
                {finding ? (
                  <TypedFound parts={typed} finding={finding} />
                ) : spans.length ? (
                  spans.map((span, index) =>
                    span.chip ? (
                      <Text key={index} style={styles.sentChip}>
                        {span.chip.label}
                      </Text>
                    ) : (
                      span.text
                    ),
                  )
                ) : (
                  text
                )}
              </Text>
            </Bubble>
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
        ) : strip && !editing ? (
          <Strip meta={strip} mine onEdit={onEdit && (() => onEdit(message))} />
        ) : !followedByOwn && !editing && message.sentAt ? (
          <Text style={styles.sentTime}>{clockLabel(message.sentAt)}</Text>
        ) : null}
      </View>
    );
  }
  return (
    <Pressable onPress={tap} onLongPress={keepLongPress} accessibilityHint={tap ? 'Tap for copy and how long it took' : undefined}>
      <Grow id={message.id} live={live}>
        <Parts id={message.id} parts={shownParts(message.parts, kept)} live={live} />
        {strip ? <Strip meta={strip} mine={false} /> : null}
      </Grow>
    </Pressable>
  );
}

/** How long the answer being written takes to open up to a new line. */
const GROW_MS = 220;

/**
 * The answer being written opens up to each new line rather than stepping, so the chat slides up
 * and what is drawn under it, such as the activity line, holds still.
 */
function Grow({ id, live, children }: { id: string; live: boolean; children: ReactNode }) {
  const still = useStill();
  const [height] = useState(() => new Animated.Value(0));
  // The list hands this row to other messages, so the height it holds counts only for the message it measured.
  const [measured, setMeasured] = useState<string | null>(null);
  const growing = live && !still && measured === id;
  const onLayout = ({ nativeEvent }: LayoutChangeEvent) => {
    const next = nativeEvent.layout.height;
    if (!growing) {
      height.setValue(next);
      setMeasured(id);
      return;
    }
    height.stopAnimation((shown) => {
      if (next <= shown) height.setValue(next);
      else Animated.timing(height, { toValue: next, duration: GROW_MS, easing: Easing.out(Easing.quad), useNativeDriver: false }).start();
    });
  };
  return (
    <Animated.View style={growing ? { height, overflow: 'hidden' } : undefined}>
      <View onLayout={onLayout}>{children}</View>
    </Animated.View>
  );
}

/** The one line a run of messages between agents folds to, as on the Mac: a tap opens it where it is. */
export function TeamFold({
  line,
  open,
  all,
  onToggle,
  onAll,
}: {
  line: TeamLine;
  open: boolean;
  all: boolean;
  onToggle: () => void;
  onAll: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const { words, wanting } = teamWords(line);
  const count = teamCount(line.messages);
  return (
    <View style={styles.team}>
      <Pressable
        onPress={() => {
          haptics.select();
          onToggle();
        }}
        style={({ pressed }) => [styles.teamRow, pressed && styles.workPressed]}
        hitSlop={6}
        accessibilityRole="button"
        accessibilityLabel={`${count}. ${words}`}
        accessibilityState={{ expanded: open }}>
        <View style={[styles.workChevron, open && { transform: [{ rotate: '90deg' }] }]}>
          <Icon name="IconChevron" size={10} color={mixed(colors.ink, colors.inkDim, 0.6)} />
        </View>
        <View style={styles.teamMarks}>
          {line.agents.map((agent) => (
            <AgentFace key={agent.agentId ?? agent.cli} agentId={agent.agentId} provider={agent.cli} size={16} />
          ))}
          {line.more > 0 ? <Text style={styles.teamMore}>+{line.more}</Text> : null}
        </View>
        <Text style={styles.teamCount}>{count}</Text>
        <Text style={[styles.teamWords, wanting && { color: colors.danger }]} numberOfLines={1}>
          {words}
        </Text>
        {line.took !== undefined ? <Text style={styles.teamTook}>{teamTook(line.took)}</Text> : null}
      </Pressable>
      {open && !all && line.earlier > 0 ? (
        <Pressable
          onPress={onAll}
          style={({ pressed }) => [styles.teamEarlier, pressed && styles.workPressed]}
          hitSlop={6}
          accessibilityRole="button">
          <Text style={styles.teamEarlierText}>Show {line.earlier} earlier</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

/** A finished answer's work, folded above the reply it closed with, as on the Mac. */
export function WorkSummary({
  took,
  calls,
  thought,
  open,
  onToggle,
}: {
  took: number | undefined;
  calls: number;
  thought: boolean;
  open: boolean;
  onToggle: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const provider = useContext(ProviderContext);
  const worked = workedLabel(took ?? null, thought);
  const counted = callsLabel(calls);
  return (
    <View style={styles.work}>
      <Pressable
        onPress={() => {
          haptics.select();
          onToggle();
        }}
        style={({ pressed }) => [styles.workSum, pressed && styles.workPressed]}
        hitSlop={6}
        accessibilityRole="button"
        accessibilityLabel={calls > 0 ? `${worked}, ${counted}` : worked}
        accessibilityState={{ expanded: open }}>
        <AgentIcon provider={provider} size={17} />
        <Text style={styles.workText}>{worked}</Text>
        {calls > 0 ? (
          <>
            <Text style={[styles.workText, styles.workDot]}>·</Text>
            <Text style={styles.workText}>{counted}</Text>
          </>
        ) : null}
        <View style={[styles.workChevron, open && { transform: [{ rotate: '90deg' }] }]}>
          <Icon name="IconChevron" size={10} color={mixed(colors.ink, colors.inkDim, 0.6)} />
        </View>
      </Pressable>
      <View style={styles.workRule} />
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    userRow: { alignItems: 'flex-end', marginTop: 20, marginBottom: 10 },
    followsOwn: { marginTop: 4 },
    followedByOwn: { marginBottom: 0 },
    sentTime: { height: 24, lineHeight: 24, fontFamily: fonts.ui, fontSize: 11, fontVariant: ['tabular-nums'], color: colors.inkFaint },
    bubble: { maxWidth: '84%' },
    letters: { gap: 14, marginTop: 14, marginBottom: 6 },
    sentChip: { color: colors.accent, backgroundColor: colors.active },
    found: { backgroundColor: translucent(colors.accent, 0.34), color: colors.ink },
    foundCurrent: { backgroundColor: colors.accent, color: colors.sunken },
    failedBubble: { borderWidth: 1, borderColor: colors.danger },
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
    queuedLabel: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkFaint, marginTop: 4 },
    team: { marginTop: 6, marginBottom: 2 },
    teamRow: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      height: 28,
      marginHorizontal: -6,
      paddingHorizontal: 6,
      borderRadius: 6,
    },
    teamMarks: { flexDirection: 'row', alignItems: 'center', gap: 2 },
    teamMore: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkDim, marginLeft: 2 },
    teamCount: { fontFamily: fonts.uiSemibold, fontSize: 12.5, color: colors.ink },
    teamWords: { flex: 1, minWidth: 0, fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    teamTook: { fontFamily: fonts.ui, fontSize: 12, fontVariant: ['tabular-nums'], color: colors.inkDim },
    teamEarlier: {
      alignSelf: 'flex-start',
      marginTop: 4,
      marginLeft: 12,
      paddingHorizontal: 6,
      height: 24,
      justifyContent: 'center',
      borderRadius: 6,
    },
    teamEarlierText: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkDim },
    work: { marginTop: 8, marginBottom: 4 },
    workSum: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      height: 24,
      alignSelf: 'flex-start',
      marginLeft: -6,
      paddingHorizontal: 6,
      borderRadius: 6,
    },
    workPressed: { backgroundColor: translucent(colors.ink, 0.06) },
    workText: { fontFamily: fonts.ui, fontSize: 12.5, color: mixed(colors.ink, colors.inkDim, 0.6) },
    workDot: { marginHorizontal: -2 },
    workRule: { height: 1, marginTop: 4, backgroundColor: translucent(colors.ink, 0.14) },
    workChevron: { opacity: 0.7 },
    strip: { flexDirection: 'row', alignItems: 'center', gap: 8, height: 24, marginTop: 6 },
    stripMine: { justifyContent: 'flex-end' },
    stripCopy: { width: 24, height: 24 },
    stripButton: { borderRadius: 7, alignItems: 'center', justifyContent: 'center' },
    stripTime: { fontFamily: fonts.ui, fontSize: 11, fontVariant: ['tabular-nums'], color: colors.inkDim },
    stripRate: { marginLeft: 'auto', fontFamily: fonts.mono, fontSize: 11, fontStyle: 'italic', color: colors.inkDim },
    prose: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 24.5, color: colors.ink },
    letterName: { flexShrink: 1, fontFamily: fonts.uiSemibold, fontSize: 12.5, color: colors.ink },
    said: { paddingTop: 10 },
    earlier: { flexDirection: 'row', alignItems: 'center', gap: 8, alignSelf: 'flex-start', height: 26 },
    earlierText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    earlierCount: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkFaint },
    thought: { marginVertical: 6 },
    thoughtText: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 24.5, color: colors.secondary },
    subagent: { marginVertical: 4, borderRadius: 10, backgroundColor: translucent(colors.raised, 0.55) },
    subagentHead: { flexDirection: 'row', alignItems: 'center', gap: 8, height: 34, paddingHorizontal: 12, borderRadius: 10 },
    subagentPressed: { backgroundColor: colors.active },
    turned: { transform: [{ rotate: '90deg' }] },
    subagentName: { flexShrink: 0, maxWidth: '35%', fontFamily: fonts.uiSemibold, fontSize: 12.5, color: colors.ink },
    subagentTask: { flexShrink: 1, minWidth: 0, fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    subagentDoing: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.secondary },
    subagentDoingLayout: { flexShrink: 1, minWidth: 0 },
    subagentEnd: { flexDirection: 'row', alignItems: 'center', gap: 6, flexShrink: 0 },
    subagentWord: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    subagentSpacer: { flex: 1 },
    subagentCalls: { flexShrink: 0, fontFamily: fonts.ui, fontSize: 12, fontVariant: ['tabular-nums'], color: colors.inkFaint },
    subagentBody: { paddingLeft: 25, paddingRight: 10, paddingTop: 2, paddingBottom: 8, gap: 2 },
    empty: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkFaint, paddingVertical: 6 },
  });
};
