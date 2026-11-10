import { memo, useMemo, useRef, useState, type ReactNode, type RefObject } from 'react';
import { ActivityIndicator, PanResponder, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';

import { unknownCommand } from '@shared/chat/composerInput';
import { currentLabel } from '@/chat/sessionConfig';
import type { ChatRows } from '@shared/chat/rows';
import type { ChatCommandInfo, PermissionAsk } from '@shared/wire.generated';
import { problem } from '@/core/problem';
import { haptics } from '@/ui/haptics';
import { AgentIcon, Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { ComposerAttachments } from './Attachments';
import { ComposerInput } from './ComposerInput';
import { AttachSheet } from './AttachSheet';
import { MAX_ATTACHMENTS } from '@shared/wire.generated';
import type { StashItem } from '@/stash/api';
import { StashMenu } from '@/stash/StashMenu';
import { StashButton } from '@/stash/StashButton';
import { StashChip } from '@/stash/StashChip';
import { usePromptStash } from '@/stash/usePromptStash';
import { stashQuery, withoutQuery, type ContextChip } from '@/stash/words';
import type { WrittenAnswer } from './questions';
import type { ChatSession } from './session';
import type { Attachment } from './uploads';
import { useDraft, useDraftTokens } from './useChat';
import { matchingCommands } from './commands';
import { typedMention } from './mentions';
import { MentionMenu } from './MentionMenu';
import { useMentions } from './useMentions';
import { PermissionDock } from '@/chat/PermissionDock';
import { ContextRing } from '@/chat/ContextRing';
import { YoloToggle } from '@/chat/YoloToggle';
import { ConfigSheet } from '@/chat/ConfigSheet';

function Commands({ matches, onPick }: { matches: ChatCommandInfo[]; onPick: (name: string) => void }) {
  const styles = useStyles(makeStyles);
  if (!matches.length) return null;
  return (
    <View style={styles.commands} accessibilityRole="menu">
      {matches.map((command) => (
        <Pressable
          key={command.name}
          onPress={() => onPick(command.name)}
          style={({ pressed }) => [styles.command, pressed && { opacity: 0.8 }]}
          accessibilityRole="menuitem"
          accessibilityLabel={`/${command.name}, ${command.description}`}>
          <Text style={styles.commandName}>/{command.name}</Text>
          <Text style={styles.commandText} numberOfLines={1}>
            {command.description}
          </Text>
        </Pressable>
      ))}
    </View>
  );
}

type ComposerQuestion = {
  name: string;
  cli: string;
  text: string;
  onRemove: () => void;
  onAnswer: (written: WrittenAnswer) => Promise<boolean>;
};

function AnsweringChip({ question }: { question: ComposerQuestion }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.answering}>
      <AgentIcon provider={question.cli} size={13} />
      <Text style={styles.answeringText} numberOfLines={1}>
        Answering {question.name} · {question.text}
      </Text>
      <Pressable
        onPress={question.onRemove}
        hitSlop={8}
        style={styles.answeringX}
        accessibilityRole="button"
        accessibilityLabel={`Stop answering ${question.name}`}>
        <Icon name="IconClose" size={10} color={colors.ground} />
      </Pressable>
    </View>
  );
}

/** How far up an empty composer is pulled before it opens the recent prompts. */
const PULL_UP = 18;

export const Composer = memo(function Composer({
  session,
  core,
  provider,
  running,
  steers,
  model,
  effort,
  usage,
  commands,
  requests,
  answering,
  placeholder,
  permissionMode,
  modeLocked,
  watchOnly,
  offline,
  hostName,
  onSent,
  attachments,
  chips,
  held,
  strip,
  onRecent,
  question,
  inputRef,
}: {
  session: ChatSession;
  /** The host the chat is on. */
  core: string;
  provider: string;
  running: boolean;
  /** Sending while the turn runs steers into it rather than waiting behind it. */
  steers: boolean;
  model: ChatRows['model'];
  effort: ChatRows['effort'];
  usage: ChatRows['usage'];
  commands: ChatCommandInfo[];
  /** The permission requests waiting on an answer, the first one shown first. */
  requests: PermissionAsk[];
  answering: ReadonlySet<string>;
  placeholder: string;
  permissionMode: string;
  /** The host would refuse a switch between YOLO and safe now. */
  modeLocked: boolean;
  watchOnly: boolean;
  /** The host is out of reach, so a message is kept until it is back. */
  offline: boolean;
  hostName: string;
  onSent: () => void;
  /** Picked for the next message. */
  attachments: readonly Attachment[];
  /** Stashed links and text picked for the next message. */
  chips: readonly ContextChip[];
  /** The agent's actions held for the person, docked while no permission request is. */
  held: ReactNode;
  /** What is still going, docked on the composer's top edge while nothing more urgent is. */
  strip: ReactNode;
  /** Pulling an empty composer up opens what was sent before; absent when nothing was. */
  onRecent?: () => void;
  /** The question an agent asked that whatever is sent answers. */
  question?: ComposerQuestion | null;
  /** Lets the screen hand the keyboard to the composer. */
  inputRef?: RefObject<TextInput | null>;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const draft = useDraft(session);
  const tokens = useDraftTokens(session);
  const [adding, setAdding] = useState<'files' | 'stash' | false>(false);
  const uploading = attachments.some((attachment) => attachment.upload === 'sending');
  const room = MAX_ATTACHMENTS - attachments.length;
  const [focused, setFocused] = useState(false);
  const ownInput = useRef<TextInput>(null);
  const input = inputRef ?? ownInput;
  const [sheet, setSheet] = useState(false);
  const modelLabel = currentLabel(model);
  const effortLabel = currentLabel(effort);
  const pulls = !draft && onRecent !== undefined;
  const pullUp = useMemo(
    () =>
      PanResponder.create({
        onMoveShouldSetPanResponderCapture: (_, gesture) => pulls && -gesture.dy > PULL_UP && -gesture.dy > Math.abs(gesture.dx) * 2,
        onPanResponderGrant: () => {
          haptics.select();
          onRecent?.();
        },
      }),
    [pulls, onRecent],
  );
  const mentions = useMentions(session, watchOnly ? undefined : typedMention(draft, tokens));
  const dock = requests.length ? (
    <PermissionDock requests={requests} provider={provider} answering={answering} onAnswer={session.answer} />
  ) : (
    held
  );

  const prompt = usePromptStash({
    stash: async () => {
      const id = await session.stashPrompt();
      input.current?.clear();
      return id;
    },
    undo: session.unstashPrompt,
    report: session.report,
  });

  if (watchOnly) {
    return (
      <SafeAreaView edges={['bottom']} style={styles.wrap}>
        {dock ?? strip}
        <View style={[styles.composer, styles.watch]}>
          <Icon name="IconEye" size={17} color={colors.inkDim} />
          <Text style={styles.watchText}>Watching</Text>
        </View>
      </SafeAreaView>
    );
  }

  const matches = matchingCommands(commands, draft);
  const mention = matches.length ? undefined : stashQuery(draft);
  const sendable = Boolean(draft.trim()) || attachments.length > 0 || chips.length > 0;
  const send = () => {
    const text = draft.trim();
    if (!sendable || offline || uploading) return;
    const unknown = unknownCommand(text, commands);
    if (unknown) {
      session.report(`/${unknown} is not available in this session`);
      return;
    }
    haptics.tap();
    const sending = question ? session.answerWith(text, question.onAnswer) : session.send(text, steers);
    void sending.then((sent) => {
      if (!sent) return;
      // Clearing the state alone leaves text the keyboard is still composing.
      input.current?.clear();
      session.setDraft('');
      onSent();
    });
  };

  const attach = (picking: Promise<Attachment[]>) => {
    picking.then(session.attach).catch((error: unknown) => session.report(`could not pick: ${problem(error)}`));
  };

  const fromStash = (items: StashItem[], base = draft) => {
    if (base !== draft) session.setDraft(base);
    session.attachStash(items).catch((error: unknown) => session.report(`could not attach it: ${problem(error)}`));
  };

  return (
    <SafeAreaView edges={['bottom']} style={styles.wrap}>
      {dock}
      <Commands matches={matches} onPick={(name) => session.setDraft(`/${name} `)} />
      {mention !== undefined ? <StashMenu query={mention} onPick={(item) => fromStash([item], withoutQuery(draft))} /> : null}
      {mentions ? (
        <MentionMenu
          menu={mentions}
          onScope={(trigger) => {
            const at = mentions.typed.start;
            session.setDraft(`${draft.slice(0, at)}${trigger}${draft.slice(at + 1)}`);
          }}
        />
      ) : null}
      {prompt.toast}
      {dock || matches.length || mention !== undefined || mentions ? null : strip}
      <View style={[styles.composer, focused && { borderColor: colors.borderSelected }]} {...pullUp.panHandlers}>
        {question ? <AnsweringChip question={question} /> : null}
        {attachments.length || chips.length ? (
          <ComposerAttachments
            attachments={attachments}
            onRemove={session.removeAttachment}
            onRetry={send}
            lead={chips.map((chip) => (
              <StashChip key={chip.uri} chip={chip} onRemove={() => session.removeChip(chip.uri)} />
            ))}
          />
        ) : null}
        <ComposerInput
          ref={input}
          value={draft}
          tokens={tokens}
          editable={!uploading}
          onChangeText={session.setDraft}
          placeholder={placeholder}
          accessibilityLabel="Message"
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
        />
        <View style={styles.bar}>
          <Pressable
            onPress={() => setAdding('files')}
            disabled={uploading || room <= 0}
            style={({ pressed }) => [
              styles.add,
              pressed && { backgroundColor: colors.active },
              (uploading || room <= 0) && { opacity: 0.4 },
            ]}
            accessibilityRole="button"
            accessibilityLabel="Add photos or files"
            accessibilityState={{ disabled: uploading || room <= 0 }}>
            <Icon name="IconPlus" size={17} color={colors.inkDim} />
          </Pressable>
          <StashButton onPress={() => setAdding('stash')} />
          <YoloToggle core={core} mode={permissionMode} provider={provider} locked={modeLocked} onToggle={session.setPermissionMode} />
          {modelLabel ? (
            <Pressable
              style={[styles.picker, styles.model]}
              onPress={() => setSheet(true)}
              accessibilityRole="button"
              accessibilityLabel={`Model, ${modelLabel}`}>
              <AgentIcon provider={provider} size={16} />
              <Text style={[styles.pickerText, styles.modelText, { color: colors.accent }]} numberOfLines={1}>
                {modelLabel}
              </Text>
              <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
                <Icon name="IconChevron" size={10} color={colors.accent} />
              </View>
            </Pressable>
          ) : null}
          {effortLabel ? (
            <Pressable
              style={styles.picker}
              onPress={() => setSheet(true)}
              accessibilityRole="button"
              accessibilityLabel={`Effort, ${effortLabel}`}>
              <Text style={styles.pickerText} numberOfLines={1}>
                {effortLabel}
              </Text>
              <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
                <Icon name="IconChevron" size={10} color={colors.inkDim} />
              </View>
            </Pressable>
          ) : null}
          {usage ? <ContextRing usage={usage} provider={provider} onPress={() => setSheet(true)} /> : null}
          <View style={{ flex: 1 }} />
          {sendable || !running ? (
            <Pressable
              onPress={send}
              onLongPress={sendable ? prompt.stash : undefined}
              style={[styles.send, (!sendable || offline) && { opacity: 0.28 }]}
              disabled={uploading}
              accessibilityRole="button"
              accessibilityLabel={
                uploading
                  ? 'Sending files'
                  : offline
                    ? `Send, waiting for ${hostName}`
                    : running
                      ? steers
                        ? 'Send to steer this turn'
                        : 'Send after this turn'
                      : 'Send'
              }
              accessibilityHint={sendable ? 'Hold to stash this prompt' : undefined}
              accessibilityState={{ disabled: offline || !sendable, busy: uploading }}>
              {uploading ? (
                <ActivityIndicator size="small" color={colors.ground} />
              ) : (
                <Icon name="IconArrowUp" size={16} color={colors.ground} />
              )}
            </Pressable>
          ) : (
            <Pressable onPress={session.cancel} style={styles.send} accessibilityRole="button" accessibilityLabel="Stop">
              <View style={styles.stop} />
            </Pressable>
          )}
        </View>
      </View>
      <AttachSheet
        visible={adding !== false}
        stash={adding === 'stash'}
        room={room}
        onClose={() => setAdding(false)}
        onAttach={attach}
        onStash={fromStash}
        onStashPrompt={sendable && !uploading ? prompt.stash : undefined}
      />
      <ConfigSheet
        visible={sheet}
        onClose={() => setSheet(false)}
        provider={provider}
        model={model}
        effort={effort}
        usage={usage}
        onPick={(picker, value) => {
          session.setConfig(picker.id, value);
          setSheet(false);
        }}
      />
    </SafeAreaView>
  );
});

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    wrap: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 6 },
    composer: { padding: 6, borderRadius: 16, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.composer },
    answering: {
      flexDirection: 'row',
      alignItems: 'center',
      alignSelf: 'flex-start',
      gap: 7,
      maxWidth: '100%',
      margin: 4,
      marginBottom: 2,
      paddingVertical: 6,
      paddingLeft: 9,
      paddingRight: 6,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
    },
    answeringText: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.inkDim },
    answeringX: { width: 20, height: 20, borderRadius: 10, backgroundColor: colors.ink, alignItems: 'center', justifyContent: 'center' },
    bar: { flexDirection: 'row', alignItems: 'center', gap: 2, paddingTop: 6 },
    add: { width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center' },
    picker: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 34, paddingHorizontal: 7, borderRadius: 7, flexShrink: 0 },
    model: { flexShrink: 1, maxWidth: 160 },
    modelText: { flexShrink: 1 },
    pickerText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    send: { width: 34, height: 34, borderRadius: 17, backgroundColor: colors.ink, alignItems: 'center', justifyContent: 'center' },
    stop: { width: 10, height: 10, borderRadius: 2, backgroundColor: colors.ground },
    watch: { flexDirection: 'row', alignItems: 'center', gap: 10, padding: 14 },
    watchText: { flex: 1, fontFamily: fonts.ui, fontSize: 13.5, lineHeight: 19, color: colors.inkDim },

    commands: { gap: 4, marginBottom: 8 },
    command: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      minHeight: 40,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
    },
    commandName: { fontFamily: fonts.mono, fontSize: 13, color: colors.accent },
    commandText: { flex: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
  });
};
