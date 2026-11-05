import { createContext, Fragment, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { Animated, Easing, Image, Pressable, StyleSheet, Text, TextInput, View, type LayoutChangeEvent } from 'react-native';

import { durationLabel } from '@shared/chat/durationLabel';
import { basename } from '@shared/lib/paths';
import { EARLIER_LOST, rateLabel, sentLabel } from '@shared/chat/messageMeta';
import type { Subagents } from '@shared/chat/subagents';
import { cutLongBlocks, showRestLabel } from '@shared/chat/longText';
import type { MdElement } from '@shared/markdown/types';
import { revealBlocks, visibleText } from '@shared/markdown/visible';
import { contextChip, type ContextChip } from '@shared/chat/promptContext';
import { sentChips } from '@shared/chat/sentChips';
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
import { callsLabel, workedLabel } from '@shared/chat/toolView';
import type {
  AccountMove,
  AgentNotice,
  ChatMessage,
  ChatPart,
  ContentBlock,
  PartTree,
  SentContext,
  SubagentCard,
  TaskNotice as Notice,
} from '@shared/chat/types';
import { useAgentNames } from '@/devices/hub';
import { CopyButton } from '@/ui/CopyButton';
import { haptics } from '@/ui/haptics';
import { AgentFace } from '@/devices/Buddy';
import { AgentIcon, Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { Working } from '@/devices/AgentMarks';
import { fonts, italic, keyboardFor, mixed, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import { useAnimating, useStill } from '@/ui/motion';
import { Shimmer } from '@/ui/Shimmer';
import { FoldsContext, useFold } from './folds';
import { FindingContext, markBlocks, markTyped, type Finding } from './found';
import { Markdown, plainBlocks } from './Markdown';
import { useStreamedText } from './reveal';
import { SentAttachments } from './Attachments';
import { hostImagePath } from './FileRef';
import { HostContext, useHostFile } from './hostFiles';
import { ImageViewer } from './ImageViewer';
import type { Strip as StripMeta } from './messageMeta';
import type { Held, Unsent } from './outbox';
import type { Attachment } from './uploads';
import { ToolGroup } from './Tools';
import { AnsweredQuestion } from './Question';

/** The chat's agent, whose mark its subagents carry. */
export const ProviderContext = createContext('agent');
/** The chat's own agent, which an answer to its own question needs no name for. */
export const AgentContext = createContext('');

export const SubagentsContext = createContext<Subagents>({ card: () => undefined, rows: {} });

type PageRef = NonNullable<Extract<ChatPart, { kind: 'tool' }>['page']>;

/**
 * A message too long to draw at once shows its start and the rest on a tap, unless the person
 * watched it stream in.
 */
function LongText({ id, text, md, live, style }: { id: string; text: string; md?: PartTree; live: boolean; style: object }) {
  const styles = useStyles(makeStyles);
  const folds = useContext(FoldsContext);
  const finding = useContext(FindingContext);
  const [whole, setWhole] = useFold(`whole/${id}`, false);
  const tree = useMemo(() => (md?.blocks as MdElement[] | undefined) ?? plainBlocks(text), [md, text]);
  const found = finding?.byPart.get(id);
  const current = finding?.current?.part === id ? finding.current.range : null;
  const blocks = useMemo(() => markBlocks(tree, found, current), [tree, found, current]);
  const plain = useMemo(() => visibleText(blocks), [blocks]);
  const streamed = useStreamedText(id, plain, live);
  if (live) folds.streamed.add(id);
  const cut = whole || found || folds.streamed.has(id) ? null : cutLongBlocks(blocks);
  const shown = streamed.text.length < plain.length ? revealBlocks(blocks, streamed.text.length) : blocks;
  return (
    <>
      {cut ? <Markdown blocks={cut.blocks} style={style} /> : <Markdown blocks={shown} fading={streamed.fading} style={style} />}
      {cut ? (
        <Pressable onPress={() => setWhole(true)} hitSlop={6} style={styles.showRest} accessibilityRole="button">
          <Text style={styles.showRestText}>{showRestLabel(cut.hidden)}</Text>
        </Pressable>
      ) : null}
    </>
  );
}

function SubagentMarkView({ state }: { state: SubagentCard['state'] }) {
  const colors = useColors();
  const mark: SubagentMark = SUBAGENT_MARKS[state];
  if (mark === 'spinner') return <Working />;
  if (mark === 'check') return <Icon name="IconCheck" size={13} color={colors.live} />;
  if (mark === 'warning') return <Icon name="IconWarning" size={12} color={colors.danger} />;
  if (mark === 'plug') return <Icon name="IconPlug" size={12} color={colors.danger} />;
  return <Icon name="IconClose" size={11} color={colors.inkFaint} />;
}

/** What a subagent did, kept to one line until it is opened; what it is doing now is over the composer. */
function Subagent({ subagent }: { subagent: SubagentCard }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const provider = useContext(ProviderContext);
  const rows = useContext(SubagentsContext).rows[subagent.sessionId];
  const [open, setOpen] = useFold(`subagent/${subagent.sessionId}`, false);
  const parts = useMemo(() => (rows ?? []).flatMap((message) => message.parts), [rows]);
  const cost = subagentCost(subagent);
  const running = subagent.state === 'running';
  return (
    <View style={styles.subagent}>
      <Pressable
        onPress={() => setOpen(!open)}
        style={({ pressed }) => [styles.subagentHead, pressed && { backgroundColor: colors.active }]}
        accessibilityRole="button"
        accessibilityLabel={`Subagent ${subagent.name}, ${SUBAGENT_WORDS[subagent.state]}${cost ? `, ${cost}` : ''}`}
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
          <Text style={[styles.subagentTask, styles.subagentTaskLayout]} numberOfLines={1}>
            {subagentTask(subagent.task)}
          </Text>
        )}
        {cost ? (
          <Text style={styles.subagentCalls} numberOfLines={1}>
            {cost}
          </Text>
        ) : null}
        <View style={styles.subagentMark}>
          <SubagentMarkView state={subagent.state} />
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

function TaskNotice({ id, notice }: { id: string; notice: Notice }) {
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
        <Icon
          name={notice.taskType === 'workflow' ? 'IconWorkflow' : 'IconTimer'}
          size={12}
          color={notice.state === 'failed' ? colors.danger : colors.inkFaint}
        />
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

function AgentNoticeLine({ notice }: { notice: AgentNotice }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const tint = notice.severity === 'error' ? colors.danger : notice.severity === 'warning' ? colors.warn : colors.inkFaint;
  return (
    <View style={styles.notice} accessible accessibilityLabel={`${notice.title}${notice.description ? `. ${notice.description}` : ''}`}>
      <View style={styles.noticeHead}>
        <Icon name={notice.severity === 'info' ? 'IconInfo' : 'IconWarning'} size={12} color={tint} />
        <Text style={[styles.noticeState, notice.severity !== 'info' && { color: tint }]}>{notice.title}</Text>
      </View>
      {notice.description ? (
        <Text style={styles.noticeSummary} selectable>
          {notice.description}
        </Text>
      ) : null}
    </View>
  );
}

function AccountLine({ move }: { move: AccountMove }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const why = move.reason === 'limit' && move.from ? `${move.from} reached its usage limit` : null;
  return (
    <View style={styles.notice} accessible accessibilityLabel={`Moved to ${move.label}${why ? `. ${why}` : ''}`}>
      <View style={styles.noticeHead}>
        <Icon name="IconUser" size={12} color={colors.inkFaint} />
        <Text style={styles.noticeState}>Moved to</Text>
        <Text style={styles.noticeName} numberOfLines={1}>
          {move.label}
        </Text>
      </View>
      {why ? <Text style={styles.noticeSummary}>{why}</Text> : null}
    </View>
  );
}

/** A page the agent showed. The phone has no web view to draw it in, so it names the page and where it is. */
function PageCard({ page }: { page: PageRef }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.resource} accessible accessibilityLabel={`Page ${page.title}, open on the computer`}>
      <Icon name="IconWindow" size={14} color={colors.inkDim} />
      <Text style={styles.pageTitle} numberOfLines={1}>
        {page.title}
      </Text>
      <Text style={styles.pageWhere}>on the computer</Text>
    </View>
  );
}

const CHIP_LOOKS: Record<NonNullable<ContextChip['kind']> | 'other', { label: string; icon: IconName }> = {
  issue: { label: 'Issue', icon: 'IconIssue' },
  pull: { label: 'Pull request', icon: 'IconPullRequest' },
  other: { label: 'Context', icon: 'IconFile' },
};

/** An issue, pull request or other item handed over with a message, by its number and title rather than its whole text. */
function ContextChips({ items }: { items: SentContext[] }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.contextChips}>
      {items.map((item) => {
        const chip = contextChip(item);
        const { label, icon } = CHIP_LOOKS[chip.kind ?? 'other'];
        return (
          <View
            key={item.uri}
            style={styles.contextChip}
            accessible
            accessibilityLabel={`${label} ${chip.number ? `#${chip.number} ` : ''}${chip.title}`}>
            <Icon name={icon} size={14} color={chip.kind ? colors.live : colors.inkFaint} />
            {chip.number ? <Text style={styles.contextNumber}>#{chip.number}</Text> : null}
            <Text style={styles.contextTitle} numberOfLines={1}>
              {chip.title}
            </Text>
          </View>
        );
      })}
    </View>
  );
}

/** A message's parts in order, with each run of tool calls drawn as one group. */
function Parts({ id, parts, live }: { id: string; parts: ChatPart[]; live: boolean }) {
  const styles = useStyles(makeStyles);
  const groups = groupParts(parts);
  return (
    <>
      {groups.map((group, index) => {
        const last = live && index === groups.length - 1;
        if ('tools' in group) return <ToolGroup key={group.id} id={`${id}/${group.id}`} parts={group.tools} live={last} />;
        const run = group.part;
        switch (run.kind) {
          case 'text':
            return run.text.trim() ? (
              <LongText key={run.id} id={run.id} text={run.text} md={run.md} live={last} style={styles.prose} />
            ) : null;
          case 'thought':
            return run.text.trim() ? (
              <View key={run.id} style={styles.thought}>
                <LongText id={run.id} text={run.text.trim()} md={run.md} live={last} style={styles.thoughtText} />
              </View>
            ) : null;
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

/** A picture in an answer, opening over the whole screen on a tap. */
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

/** An image or file the agent put in its answer. */
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

/** A long press is the text's own, for selecting it, so it must not also count as a tap. */
const keepLongPress = () => {};

/** The host's agent names, for the `@` chips a sent message wrote. */
function useNamesIn(core: string): string[] {
  const names = useAgentNames(core);
  return useMemo(() => (names ? names.split('\n') : []), [names]);
}

/** Writing a sent message again where its bubble was. */
export type Editing = { onCancel: () => void; onSend: (text: string) => void };

function MessageEditor({ text, editing }: { text: string; editing: Editing }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [draft, setDraft] = useState(text);
  const words = draft.trim();
  return (
    <View style={styles.editor}>
      <TextInput
        value={draft}
        onChangeText={setDraft}
        multiline
        autoFocus
        style={styles.editorText}
        selectionColor={colors.accent}
        keyboardAppearance={keyboardFor(colors)}
        accessibilityLabel="Edit message"
      />
      <View style={styles.editorActs}>
        <Pressable
          onPress={editing.onCancel}
          hitSlop={6}
          style={({ pressed }) => [styles.editorButton, pressed && { backgroundColor: colors.active }]}
          accessibilityRole="button">
          <Text style={styles.editorButtonText}>Cancel</Text>
        </Pressable>
        <Pressable
          onPress={() => editing.onSend(words)}
          disabled={!words}
          hitSlop={6}
          style={({ pressed }) => [styles.editorButton, styles.editorSend, (pressed || !words) && { opacity: 0.6 }]}
          accessibilityRole="button"
          accessibilityState={{ disabled: !words }}>
          <Text style={[styles.editorButtonText, styles.editorSendText]}>Send</Text>
        </Pressable>
      </View>
    </View>
  );
}

export function Message({
  message,
  live = false,
  unsent,
  onRetry,
  onTap,
  strip,
  sentFiles,
  from = null,
  onEdit,
  editing,
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
  from?: number | null;
  /** Writes this message again; absent when the chat cannot take it back. */
  onEdit?: (message: ChatMessage) => void;
  /** This message is being written again. */
  editing?: Editing;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const chatAgent = useContext(AgentContext);
  const host = useContext(HostContext);
  const agentNames = useNamesIn(message.role === 'user' ? host : '');
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
      <Pressable onPress={tap} onLongPress={keepLongPress} accessibilityHint={tap ? 'Tap for copy and the time it came' : undefined}>
        {letters.map((letter, index) =>
          letter.trigger ? (
            <FiredTrigger key={index} id={`${message.id}/letter/${index}`} fired={letter.trigger} />
          ) : letter.kind === 'answer' ? (
            <AnsweredQuestion
              key={index}
              own={letter.agentId === chatAgent}
              asker={letter.name}
              cli={letter.cli}
              question={letter.question ?? ''}
              answer={letter.text}
              secret={letter.secret ?? false}
            />
          ) : (
            <View key={index}>
              <View style={styles.letterFrom}>
                <View style={styles.letterRule} />
                <AgentFace agentId={letter.agentId} provider={letter.cli} size={13} />
                <Text style={styles.letterLabel}>
                  Message from <Text style={styles.letterName}>{letter.name}</Text>
                </Text>
                <View style={styles.letterRule} />
              </View>
              <LongText id={`${message.id}/letter/${index}`} text={letter.text} md={letter.tree} live={false} style={styles.prose} />
            </View>
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
    const { spans, drawn } = sentChips(text, message.attachments ?? [], message.context ?? [], agentNames);
    const context = (message.context ?? []).filter((item) => !drawn.has(item.uri));
    return (
      <View style={styles.userRow}>
        {message.attachments?.length ? <SentAttachments paths={message.attachments} sentFiles={sentFiles} /> : null}
        {context.length ? <ContextChips items={context} /> : null}
        {editing ? (
          <MessageEditor key={message.id} text={text} editing={editing} />
        ) : text || (!message.attachments?.length && !context.length) ? (
          <Pressable
            onPress={tap}
            onLongPress={keepLongPress}
            style={[styles.bubble, unsent?.state === 'sending' && { opacity: 0.7 }, failed && styles.failedBubble]}
            accessibilityHint={tap ? 'Tap for copy and the time it was sent' : undefined}>
            <Text style={styles.userText} selectable>
              {finding ? (
                <TypedFound parts={typed} finding={finding} />
              ) : (
                spans.map((span, index) =>
                  typeof span === 'string' ? (
                    span
                  ) : (
                    <Text key={index} style={styles.sentChip}>
                      {span.chip.label}
                    </Text>
                  ),
                )
              )}
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
        ) : strip && !editing ? (
          <Strip meta={strip} mine onEdit={onEdit && (() => onEdit(message))} />
        ) : null}
      </View>
    );
  }
  return (
    <Pressable onPress={tap} onLongPress={keepLongPress} accessibilityHint={tap ? 'Tap for copy and how long it took' : undefined}>
      <Grow id={message.id} live={live}>
        <Parts id={message.id} parts={shownParts(message.parts, from)} live={live} />
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

/** Messages waiting behind the running turn, faded where they will go. */
export function Queued({ held, sentFiles }: { held: readonly Held[]; sentFiles: ReadonlyMap<string, Attachment> }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.userRow}>
      {held.map((message) => {
        const { paths } = message;
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
export function Earlier({ failed, loading, onRetry }: { failed: boolean; loading: boolean; onRetry: () => void }) {
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
  // The row waits above a long chat's first message; an idle spinner there would redraw the screen every frame.
  return (
    <View style={styles.earlier}>
      {loading ? <Working /> : <View style={styles.earlierIdle} />}
      <Text style={styles.earlierText}>Loading earlier messages…</Text>
    </View>
  );
}

/** Stands above a chat whose host no longer keeps its first turns. */
export function EarlierLost() {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.earlier}>
      <Text style={styles.earlierText}>{EARLIER_LOST}</Text>
    </View>
  );
}

/** The working line: the agent's logo breathing beside what it is doing, and for how long. */
export function Activity({ provider, label, since }: { provider: string; label: string; since: number }) {
  const styles = useStyles(makeStyles);
  const [breath] = useState(() => new Animated.Value(1));
  const [seconds, setSeconds] = useState(() => Math.round((Date.now() - since) / 1000));
  const animating = useAnimating();
  useEffect(() => {
    if (!animating) return;
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
  }, [breath, since, animating]);
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
    sentChip: { color: colors.accent, backgroundColor: colors.active },
    found: { backgroundColor: translucent(colors.accent, 0.34), color: colors.ink },
    foundCurrent: { backgroundColor: colors.accent, color: colors.sunken },
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
    contextChips: { flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'flex-end', gap: 6, maxWidth: '84%', marginBottom: 6 },
    contextChip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      maxWidth: 240,
      paddingVertical: 7,
      paddingHorizontal: 10,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    contextNumber: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.ink },
    contextTitle: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.inkDim },
    resourceText: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 12, color: colors.ink },
    pageTitle: { flexShrink: 1, fontFamily: fonts.uiMedium, fontSize: 13, color: colors.ink },
    pageWhere: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    retry: { color: colors.ink, fontFamily: fonts.uiMedium },
    queued: { alignItems: 'flex-end', alignSelf: 'stretch', opacity: 0.55, marginBottom: 6 },
    queuedLabel: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkFaint, marginTop: 4 },
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
    editor: {
      width: '84%',
      gap: 8,
      paddingTop: 9,
      paddingBottom: 8,
      paddingHorizontal: 13,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.borderSelected,
      backgroundColor: colors.raised,
    },
    editorText: { fontFamily: fonts.ui, fontSize: 14, lineHeight: 22, color: colors.ink, maxHeight: 264, padding: 0 },
    editorActs: { flexDirection: 'row', justifyContent: 'flex-end', gap: 6 },
    editorButton: {
      height: 30,
      paddingHorizontal: 12,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      alignItems: 'center',
      justifyContent: 'center',
    },
    editorButtonText: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.ink },
    editorSend: { backgroundColor: colors.ink, borderColor: colors.ink },
    editorSendText: { color: colors.sunken },
    stripTime: { fontFamily: fonts.ui, fontSize: 11, fontVariant: ['tabular-nums'], color: colors.inkDim },
    stripRate: { marginLeft: 'auto', fontFamily: fonts.mono, fontSize: 11, fontStyle: 'italic', color: colors.inkDim },
    prose: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 23, color: colors.ink },
    letterFrom: { flexDirection: 'row', alignItems: 'center', gap: 8, marginTop: 16, marginBottom: 2 },
    letterRule: { flex: 1, height: StyleSheet.hairlineWidth, backgroundColor: colors.border },
    letterLabel: { fontFamily: fonts.ui, fontSize: 11.5, lineHeight: 16, color: colors.inkDim },
    triggerFrom: { flexDirection: 'row', alignItems: 'center', gap: 5, marginBottom: 6, maxWidth: '85%' },
    triggerTitle: { fontFamily: fonts.uiMedium, color: colors.ink, flexShrink: 1 },
    requestLine: { flexDirection: 'row', alignItems: 'center', gap: 6, marginTop: 6, paddingVertical: 4, paddingHorizontal: 8 },
    requestSummary: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkFaint, flexShrink: 1 },
    requestCard: {
      alignSelf: 'stretch',
      marginTop: 4,
      padding: 10,
      gap: 8,
      borderWidth: StyleSheet.hairlineWidth,
      borderColor: colors.border,
      borderRadius: 8,
      backgroundColor: colors.raised,
    },
    requestHeader: { fontFamily: fonts.mono, fontSize: 11, lineHeight: 17, color: colors.ink },
    requestName: { color: colors.inkFaint },
    letterName: { fontFamily: fonts.uiSemibold, color: colors.ink },
    thought: { marginVertical: 8 },
    thoughtText: { ...italic, fontSize: 12.5, lineHeight: 19.5, color: colors.inkFaint },
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
    subagentName: { flexShrink: 1, maxWidth: '45%', fontFamily: fonts.mono, fontSize: 11.5, color: colors.ink },
    subagentTask: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    subagentTaskLayout: { flex: 1, minWidth: 0 },
    subagentCalls: { flexShrink: 0, fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint },
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
    earlierIdle: { width: 13, height: 13 },
    earlierText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    activity: { flexDirection: 'row', alignItems: 'center', gap: 9, marginTop: 14, marginBottom: 6 },
    activityText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkFaint },
    activityLabel: { flexShrink: 1 },
    activityTime: { fontFamily: fonts.mono, fontSize: 10, color: colors.inkFaint },
  });
};

type Fired = NonNullable<NonNullable<ChatMessage['letters']>[number]['trigger']>;

/** A trigger's prompt where the person's own messages sit, under a line naming the trigger, with the
 *  webhook request that fired it folded beneath. */
function FiredTrigger({ id, fired }: { id: string; fired: Fired }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [open, setOpen] = useFold(`${id}/request`, false);
  const request = fired.request;
  const kind = request?.headers
    .find((header) => header.name === 'content-type')
    ?.value.split(';')[0]
    ?.trim();
  const size = request ? (request.body.length < 1024 ? `${request.body.length} B` : `${(request.body.length / 1024).toFixed(1)} KB`) : '';
  const summary = request ? [request.method, kind, size, request.query ? `?${request.query}` : undefined].filter(Boolean).join(' · ') : '';
  return (
    <View style={styles.userRow}>
      <View style={styles.triggerFrom}>
        <Icon name="IconTimer" size={12} color={colors.cmd} />
        <Text style={styles.letterLabel} numberOfLines={1}>
          Trigger · <Text style={styles.triggerTitle}>{fired.title}</Text>
        </Text>
      </View>
      <View style={styles.bubble}>
        <Text style={styles.userText} selectable>
          {fired.prompt}
        </Text>
      </View>
      {request ? (
        <>
          <Pressable
            onPress={() => setOpen(!open)}
            style={styles.requestLine}
            accessibilityRole="button"
            accessibilityState={{ expanded: open }}
            accessibilityLabel={`The request that fired it: ${summary}`}>
            <View style={open ? { transform: [{ rotate: '90deg' }] } : undefined}>
              <Icon name="IconChevron" size={10} color={colors.inkFaint} />
            </View>
            <Text style={styles.requestSummary} numberOfLines={1}>
              {summary}
            </Text>
          </Pressable>
          {open ? (
            <View style={styles.requestCard}>
              <View>
                {request.headers.map((header) => (
                  <Text key={header.name} style={styles.requestHeader} selectable>
                    <Text style={styles.requestName}>{header.name}</Text> {header.value}
                  </Text>
                ))}
              </View>
              {request.bodyTree ? (
                <LongText id={`${id}/request/body`} text={request.body} md={request.bodyTree} live={false} style={styles.prose} />
              ) : null}
            </View>
          ) : null}
        </>
      ) : null}
    </View>
  );
}
