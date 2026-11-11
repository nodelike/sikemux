import { useEffect, useMemo, useRef, useState } from 'react';
import { KeyboardAvoidingView, Pressable, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { router, useLocalSearchParams } from 'expo-router';
import { File } from 'expo-file-system';

import { composerPlaceholder } from '@shared/chat/chatStatus';
import type { ChatMessage, LauncherInfo } from '@shared/wire.generated';
import { agentLabel } from '@shared/agents/catalog';
import { ComposerAttachments } from '@/chat/Attachments';
import { ChatTitle } from '@/chat/ChatTitle';
import { newChatDraft, type NewChatDraft } from '@/chat/kept';
import { Message } from '@/chat/Transcript';
import { Activity } from '@/chat/Activity';
import { startChat } from '@/chat/start';
import { AttachSheet } from '@/chat/AttachSheet';
import { YoloEdge, YoloToggle } from '@/chat/YoloToggle';
import { WorktreeSwitch } from '@/chat/Worktree';
import { ComposerInput } from '@/chat/ComposerInput';
import { fitting, type Attachment } from '@/chat/uploads';
import { dropTag, tagTokens, writeTags } from '@/chat/imageTags';
import { api } from '@/account/api';
import { stashPrompt, type StashItem } from '@/stash/api';
import { downloadedFile, intoMessage, stashIdsOf } from '@/stash/attach';
import { StashButton } from '@/stash/StashButton';
import { StashChip } from '@/stash/StashChip';
import { StashMenu } from '@/stash/StashMenu';
import { stashToken, takeStash } from '@/stash/store';
import { usePromptStash } from '@/stash/usePromptStash';
import { contextChip, stashQuery, withContext, withoutQuery } from '@/stash/words';
import { MAX_ATTACHMENTS } from '@shared/wire.generated';
import { problem as problemOf } from '@/core/problem';
import { callHost, useHostCall } from '@/core/hostCalls';
import { SmartPick } from '@/chat/Notices';
import { useDevices, useLive } from '@/devices/hub';
import { deviceName } from '@/devices/paired';
import { ProjectSheet } from '@/devices/ProjectSheet';
import { haptics } from '@/ui/haptics';
import { AgentFace } from '@/devices/Buddy';
import { AgentIcon, Icon } from '@/ui/Icon';
import { type Glyph, PromptGlyph } from '@/ui/prompt';
import { Nav, Screen, useKeyboardShown } from '@/ui/screen';
import { Working } from '@/devices/AgentMarks';
import { Sheet, SheetLabel } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles, useType } from '@/ui/theme';

/** How long typing rests before the new chat's draft is written down. */
const DRAFT_KEEP_MS = 400;

function keepNewChat(store: ReturnType<typeof newChatDraft>, held: NewChatDraft) {
  if (held.text || held.attachments.length || held.chips.length) store.save(held);
  else store.clear();
}

const IDLE = composerPlaceholder({ connection: 'ready', running: false }, { resuming: false, disconnected: false });

/** The account a launcher runs on, as the core names it after the agent: "Claude · Work" is Work. */
const accountName = (launcher: LauncherInfo) => {
  const prefix = `${agentLabel(launcher.provider)} · `;
  return launcher.label.startsWith(prefix) ? launcher.label.slice(prefix.length) : launcher.label;
};

/** What was sent, drawn as the chat will draw it while the agent starts. */
type Sending = { message: ChatMessage; draft: string; attachments: Attachment[]; files: ReadonlyMap<string, Attachment>; since: number };

function sendingOf(draft: string, attachments: Attachment[]): Sending {
  const text = draft.trim();
  const since = Date.now();
  return {
    message: {
      id: 'starting',
      role: 'user',
      sentAt: since,
      parts: text ? [{ id: 'starting-text', kind: 'text', text }] : [],
      ...(attachments.length ? { attachments: attachments.map((attachment) => attachment.uri) } : {}),
    },
    draft,
    attachments,
    files: new Map(attachments.map((attachment) => [attachment.uri, attachment])),
    since,
  };
}

function AgentSheet({
  visible,
  onClose,
  launchers,
  chosen,
  onChoose,
}: {
  visible: boolean;
  onClose: () => void;
  launchers: LauncherInfo[];
  chosen?: LauncherInfo;
  onChoose: (launcher: LauncherInfo) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const agents = launchers.filter((launcher, index) => launchers.findIndex((first) => first.provider === launcher.provider) === index);
  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetLabel>Agent</SheetLabel>
      <View style={styles.agents}>
        {agents.map((launcher) => {
          const on = launcher.provider === chosen?.provider;
          return (
            <Pressable
              key={launcher.provider}
              onPress={() => onChoose(on && chosen ? chosen : launcher)}
              style={[styles.agent, on && styles.agentOn]}
              accessibilityRole="button"
              accessibilityState={{ selected: on }}>
              <AgentIcon provider={launcher.provider} size={22} />
              <Text style={[styles.agentText, on && { color: colors.ink }]} numberOfLines={1}>
                {agentLabel(launcher.provider)}
              </Text>
            </Pressable>
          );
        })}
      </View>
    </Sheet>
  );
}

function SettingsSheet({
  visible,
  onClose,
  accounts,
  chosen,
  onChoose,
}: {
  visible: boolean;
  onClose: () => void;
  accounts: LauncherInfo[];
  chosen: LauncherInfo;
  onChoose: (launcher: LauncherInfo) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.sheetHead}>
        <AgentIcon provider={chosen.provider} size={20} />
        <Text style={styles.sheetTitle}>{agentLabel(chosen.provider)}</Text>
      </View>
      <SheetLabel>Account</SheetLabel>
      <View style={styles.group}>
        {accounts.map((launcher, index) => {
          const on = launcher.id === chosen.id;
          return (
            <Pressable
              key={launcher.id}
              onPress={() => onChoose(launcher)}
              style={[styles.option, on && { backgroundColor: colors.active }, index > 0 && styles.optionDivided]}
              accessibilityRole="button"
              accessibilityState={{ selected: on }}>
              <AgentIcon provider={launcher.provider} size={18} />
              <Text style={[styles.optionText, on && { color: colors.ink }]} numberOfLines={1}>
                {accountName(launcher)}
              </Text>
              {on ? <Icon name="IconCheck" size={17} color={colors.ink} /> : null}
            </Pressable>
          );
        })}
      </View>
    </Sheet>
  );
}

export { Crashed as ErrorBoundary } from '@/ui/Crashed';

export default function NewChat() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const {
    core,
    project: linkedProject,
    launcher: linkedLauncher,
  } = useLocalSearchParams<{ core: string; project?: string; launcher?: string }>();
  const live = useLive(core);
  const { devices } = useDevices();
  const known = devices.find((device) => device.core === core);
  const hostName = known ? deviceName(known) : 'the host';
  const [focused, setFocused] = useState(false);
  /** Where the caret sits in the draft, where a picked picture's tag goes. */
  const caret = useRef<number | undefined>(undefined);
  const workspace = live.snapshot?.workspace;
  const [launcherId, setLauncherId] = useState<string | undefined>(linkedLauncher);
  const [chosenMode, setChosenMode] = useState<string>();
  const [projectId, setProjectId] = useState<string | undefined>(linkedProject);
  const kept = useMemo(() => newChatDraft(core), [core]);
  const [held] = useState(() => kept.load());
  const [draft, setDraft] = useState(held?.text ?? '');
  const [sheet, setSheet] = useState<'agent' | 'project' | 'attach' | 'stash' | 'settings'>();
  const [attachments, setAttachments] = useState<Attachment[]>(
    () => held?.attachments.filter((attachment) => new File(attachment.uri).exists) ?? [],
  );
  const [chips, setChips] = useState<StashItem[]>(held?.chips ?? []);
  const [sending, setSending] = useState<Sending>();
  const starting = Boolean(sending);
  const [problem, setProblem] = useState<string>();
  const [started, setStarted] = useState<string>();
  /** What the person flipped the worktree switch to for a project, until the host's own view says so. */
  const [flipped, setFlipped] = useState<{ project: string; on: boolean }>();
  /** Where the first message is while the host makes its worktree and starts the agent there. */
  const [step, setStep] = useState<string>();
  /** The host made this chat's worktree, so sending again starts the agent there rather than making another. */
  const [inWorktree, setInWorktree] = useState(false);
  const typing = useKeyboardShown();
  const launcher = useMemo(
    () => workspace?.launchers.find((known) => known.id === launcherId) ?? workspace?.launchers[0],
    [workspace, launcherId],
  );
  const accounts = useMemo(
    () => workspace?.launchers.filter((known) => known.provider === launcher?.provider) ?? [],
    [workspace, launcher],
  );
  const project = useMemo(
    () => workspace?.projects.find((known) => known.id === projectId) ?? workspace?.projects[0],
    [workspace, projectId],
  );
  const mode = chosenMode ?? launcher?.permissionMode ?? '';
  const worktree = (flipped?.project === project?.id ? flipped?.on : undefined) ?? project?.worktreeDefault ?? false;
  const flipWorktree = () => {
    if (!project || live.status !== 'open') return;
    const on = !worktree;
    setFlipped({ project: project.id, on });
    callHost(live.connection, 'setWorktreeDefault', { project: project.path, on }).catch((error: unknown) => {
      setFlipped(undefined);
      setProblem(`Could not change the worktree default: ${problemOf(error)}`);
    });
  };
  const room = MAX_ATTACHMENTS - attachments.length;
  const mention = stashQuery(draft);
  const stashable = Boolean(draft.trim() || attachments.length || chips.length) && !starting;
  /** What the composer held when its prompt went to the stash, to put back on Undo. */
  const stashedFrom = useRef<{ draft: string; attachments: Attachment[]; chips: StashItem[] } | null>(null);
  const prompt = usePromptStash({
    stash: async () => {
      const token = stashToken();
      if (!token) throw new Error('Sign in again.');
      const held = { draft, attachments, chips };
      stashedFrom.current = held;
      setDraft('');
      setAttachments([]);
      setChips([]);
      try {
        const text = withContext(held.draft.trim(), held.chips.map(contextChip)) || held.attachments.map((file) => file.name).join(', ');
        return (await stashPrompt(token, { text, files: held.attachments })).id;
      } catch (error) {
        putBack(held);
        throw error;
      }
    },
    undo: async (id) => {
      const token = stashToken();
      const held = stashedFrom.current;
      if (held) putBack(held);
      if (token) await api(token).deleteStash(id);
    },
    report: setProblem,
  });
  const putBack = (held: { draft: string; attachments: Attachment[]; chips: StashItem[] }) => {
    setDraft((now) => [held.draft, now].filter(Boolean).join('\n\n'));
    setAttachments((now) => [...held.attachments, ...now]);
    setChips((now) => [...held.chips, ...now.filter((chip) => !held.chips.some((known) => known.id === chip.id))]);
  };
  const sendable = Boolean(
    (draft.trim() || attachments.length || chips.length) && launcher && project && live.status === 'open' && !starting,
  );
  const blocked =
    live.status !== 'open'
      ? live.snapshot
        ? 'Reconnecting to the host…'
        : 'Not connected to the host yet.'
      : workspace && !project
        ? 'No open projects on the host.'
        : undefined;
  const noAgents = Boolean(workspace && !workspace.launchers.length);
  const absent: Glyph | undefined = noAgents ? 'IconAgent' : live.status === 'open' && workspace && !project ? 'IconFolder' : undefined;
  const busy = useRef(false);

  const unsent = useRef<NewChatDraft>(undefined);
  useEffect(() => {
    const now = { text: draft, attachments, chips };
    unsent.current = now;
    const timer = setTimeout(() => keepNewChat(kept, now), DRAFT_KEEP_MS);
    return () => clearTimeout(timer);
  }, [kept, draft, attachments, chips]);
  useEffect(
    () => () => {
      if (!busy.current && unsent.current) keepNewChat(kept, unsent.current);
    },
    [kept],
  );

  const attach = (picking: Promise<Attachment[]>) => {
    picking
      .then((picked) => {
        const { fits, notice } = fitting(attachments.length, picked);
        if (notice) setProblem(notice);
        const held = attachments.length;
        setAttachments((now) => [...now, ...fits]);
        // Each picture is named in the words at the caret, as `[Image #n]`, where the host puts it.
        const numbers = fits.flatMap((file, index) => (file.kind === 'image' ? [held + index + 1] : []));
        setDraft((now) => writeTags(now, caret.current, numbers));
      })
      .catch((error: unknown) => setProblem(`Could not pick: ${problemOf(error)}`));
  };

  const fromStash = (items: StashItem[], base = draft) => {
    const put = intoMessage(items, base, downloadedFile);
    setDraft(put.draft);
    setChips((known) => [...known, ...put.chips.filter((chip) => !known.some((held) => held.id === chip.id))]);
    if (put.files) attach(put.files);
    void takeStash(put.restored);
  };

  const smartPick = useHostCall(
    launcher && project && live.status === 'open' ? live.connection : undefined,
    'smartPick',
    { launcher: launcher?.id ?? '', project: project?.id ?? '' },
    '',
  ).data;

  const start = async () => {
    const text = withContext(draft.trim(), chips.map(contextChip));
    if (busy.current || (!text && !attachments.length) || !launcher || !project || live.status !== 'open') return;
    busy.current = true;
    haptics.tap();
    const shown = sendingOf(draft, attachments);
    setSending(shown);
    setDraft('');
    setAttachments([]);
    setProblem(undefined);
    const outcome = await startChat(
      live.connection,
      {
        core,
        launcherId: launcher.id,
        projectId: project.id,
        mode: chosenMode,
        text,
        attachments: shown.attachments,
        worktree,
        agentId: started,
        inWorktree,
      },
      { started: setStarted, step: setStep },
    );
    if (outcome.kind === 'failed') {
      busy.current = false;
      haptics.failure();
      setInWorktree(outcome.inWorktree);
      setProblem(outcome.problem);
      setDraft(shown.draft);
      setAttachments(outcome.files);
      setSending(undefined);
      setStep(undefined);
      return;
    }
    void takeStash(stashIdsOf(shown.attachments, chips));
    setChips([]);
    kept.clear();
    router.replace({ pathname: '/device/[core]/chat/[agent]', params: { core, agent: outcome.agentId, started: '1' } });
  };

  return (
    <Screen>
      <Nav
        title={launcher ? <ChatTitle provider={launcher.provider} title={`New ${agentLabel(launcher.provider)} chat`} /> : undefined}
        end={
          accounts.length > 1 && !started ? (
            <Pressable
              onPress={() => setSheet('settings')}
              hitSlop={6}
              style={({ pressed }) => [styles.more, pressed && { backgroundColor: colors.active }]}
              accessibilityRole="button"
              accessibilityLabel="Chat settings">
              <Icon name="IconMoreVertical" size={16} color={colors.tertiary} />
            </Pressable>
          ) : undefined
        }
      />
      <KeyboardAvoidingView style={{ flex: 1 }} behavior="padding">
        {sending ? (
          <View style={styles.sending}>
            <Message message={sending.message} unsent={{ state: 'sending' }} sentFiles={sending.files} />
            {launcher ? <Activity provider={launcher.provider} doing={{ verb: step ?? 'Starting' }} since={sending.since} /> : null}
          </View>
        ) : (
          <View style={styles.welcome}>
            {absent ? (
              <PromptGlyph icon={absent} small />
            ) : launcher ? (
              <AgentFace agentId={project?.id} provider={launcher.provider} size={38} logoSize={40} />
            ) : null}
            <Text style={styles.welcomeTitle}>New chat</Text>
            {launcher && !noAgents && !problem && !blocked ? (
              <Text style={type.meta}>
                {agentLabel(launcher.provider)} on {hostName}
              </Text>
            ) : null}
            {smartPick && launcher && !problem && !blocked ? <SmartPick move={smartPick} provider={launcher.provider} /> : null}
            {noAgents ? <Text style={[type.meta, { textAlign: 'center' }]}>No agents on the host.</Text> : null}
            {problem ? <Text style={styles.problem}>{problem}</Text> : null}
            {!problem && blocked ? (
              <Text style={[type.meta, { textAlign: 'center' }]} accessibilityLiveRegion="polite">
                {blocked}
              </Text>
            ) : null}
          </View>
        )}
        <SafeAreaView edges={typing ? [] : ['bottom']} style={styles.wrap}>
          {prompt.toast}
          {mention !== undefined ? <StashMenu query={mention} onPick={(item) => fromStash([item], withoutQuery(draft))} /> : null}
          <View style={[styles.composer, mode === 'bypass' ? styles.yoloComposer : focused && { borderColor: colors.borderSelected }]}>
            {mode === 'bypass' ? <YoloEdge radius={10} /> : null}
            {attachments.length || chips.length ? (
              <ComposerAttachments
                attachments={attachments}
                onRemove={(id) => {
                  const at = attachments.findIndex((attachment) => attachment.id === id);
                  if (starting || at < 0) return;
                  setAttachments((now) => now.filter((attachment) => attachment.id !== id));
                  setDraft((now) => dropTag(now, at + 1));
                }}
                onRetry={start}
                lead={chips.map((chip) => (
                  <StashChip
                    key={chip.id}
                    chip={contextChip(chip)}
                    onRemove={() => setChips((now) => now.filter((held) => held.id !== chip.id))}
                  />
                ))}
              />
            ) : null}
            <ComposerInput
              value={draft}
              tokens={tagTokens(draft, attachments.length)}
              onChangeText={setDraft}
              placeholder={IDLE}
              editable={!starting}
              autoFocus
              onFocus={() => setFocused(true)}
              onBlur={() => setFocused(false)}
              onSelectionChange={(event) => {
                caret.current = event.nativeEvent.selection.start;
              }}
            />
            <View style={styles.bar}>
              <Pressable
                onPress={() => setSheet('attach')}
                disabled={starting || room <= 0}
                style={({ pressed }) => [
                  styles.add,
                  pressed && { backgroundColor: colors.active },
                  (starting || room <= 0) && { opacity: 0.4 },
                ]}
                accessibilityRole="button"
                accessibilityLabel="Add photos or files"
                accessibilityState={{ disabled: starting || room <= 0 }}>
                <Icon name="IconPlus" size={17} color={colors.inkDim} />
              </Pressable>
              <StashButton onPress={() => setSheet('stash')} />
              {launcher ? (
                <YoloToggle
                  core={core}
                  mode={mode}
                  provider={launcher.provider}
                  locked={starting || Boolean(started)}
                  onToggle={setChosenMode}
                />
              ) : null}
              {launcher ? (
                <Pressable
                  style={styles.picker}
                  onPress={() => setSheet('agent')}
                  disabled={Boolean(started)}
                  hitSlop={5}
                  accessibilityRole="button"
                  accessibilityLabel="Agent">
                  <AgentIcon provider={launcher.provider} size={16} />
                  <Text style={styles.pickerText}>{agentLabel(launcher.provider)}</Text>
                  <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
                    <Icon name="IconChevron" size={10} color={colors.accent} />
                  </View>
                </Pressable>
              ) : null}
              <View style={{ flex: 1 }} />
              <Pressable
                onPress={start}
                onLongPress={stashable ? prompt.stash : undefined}
                disabled={!sendable && !stashable}
                hitSlop={5}
                style={[styles.send, !sendable && { opacity: 0.28 }]}
                accessibilityRole="button"
                accessibilityLabel="Start"
                accessibilityHint={blocked}
                accessibilityState={{ disabled: !sendable, busy: starting }}>
                {starting ? <Working /> : <Icon name="IconArrowUp" size={16} color={colors.ground} />}
              </Pressable>
            </View>
          </View>
          {project ? (
            <View style={styles.place}>
              <View style={styles.placeHost}>
                <Icon name="IconLaptop" size={12} color={colors.tertiary} />
                <Text style={styles.placeQuiet} numberOfLines={1}>
                  {hostName}
                </Text>
              </View>
              <Text style={styles.slash} importantForAccessibility="no" accessibilityElementsHidden>
                /
              </Text>
              <Pressable
                style={styles.placeProject}
                onPress={() => setSheet('project')}
                disabled={Boolean(started)}
                hitSlop={6}
                accessibilityRole="button"
                accessibilityLabel={`Project, ${project.name}`}>
                <Icon name="IconFolder" size={12} color={colors.live} />
                <Text style={styles.placeName} numberOfLines={1}>
                  {project.name}
                </Text>
                <View style={{ transform: [{ rotate: '90deg' }] }}>
                  <Icon name="IconChevron" size={10} color={colors.inkFaint} />
                </View>
              </Pressable>
              <View style={{ flex: 1 }} />
              {project.git ? <WorktreeSwitch on={worktree || inWorktree} disabled={starting || inWorktree} onFlip={flipWorktree} /> : null}
            </View>
          ) : null}
        </SafeAreaView>
      </KeyboardAvoidingView>
      <AttachSheet
        visible={sheet === 'attach' || sheet === 'stash'}
        stash={sheet === 'stash'}
        room={room}
        onClose={() => setSheet(undefined)}
        onAttach={attach}
        onStash={fromStash}
        onStashPrompt={stashable ? prompt.stash : undefined}
      />
      <AgentSheet
        visible={sheet === 'agent'}
        onClose={() => setSheet(undefined)}
        launchers={workspace?.launchers ?? []}
        chosen={launcher}
        onChoose={(next) => {
          setLauncherId(next.id);
          setChosenMode(undefined);
          setSheet(undefined);
        }}
      />
      {launcher ? (
        <SettingsSheet
          visible={sheet === 'settings'}
          onClose={() => setSheet(undefined)}
          accounts={accounts}
          chosen={launcher}
          onChoose={(next) => {
            setLauncherId(next.id);
            setSheet(undefined);
          }}
        />
      ) : null}
      <ProjectSheet
        visible={sheet === 'project'}
        onClose={() => setSheet(undefined)}
        projects={workspace?.projects ?? []}
        chosen={project?.id ?? null}
        onChoose={(next) => {
          if (next) setProjectId(next);
          setSheet(undefined);
        }}
      />
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    welcome: {
      flex: 1,
      alignItems: 'center',
      // Nothing floats mid-screen: the start sits just above the composer it leads into.
      justifyContent: 'flex-end',
      gap: 10,
      paddingHorizontal: 28,
      paddingBottom: 28,
    },
    sending: { flex: 1, justifyContent: 'flex-end', paddingHorizontal: 18, paddingBottom: 28 },
    welcomeTitle: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.4, color: colors.ink },
    problem: { fontFamily: fonts.ui, fontSize: 13.5, color: colors.danger, textAlign: 'center' },
    wrap: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 6 },
    // Where the chat will run, picked from the line under the box as on the Mac's new chat.
    place: { flexDirection: 'row', alignItems: 'center', gap: 4, minHeight: 36, marginTop: 4, paddingHorizontal: 8 },
    placeHost: { flexShrink: 1, flexDirection: 'row', alignItems: 'center', gap: 6, paddingHorizontal: 4 },
    placeQuiet: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    placeProject: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingHorizontal: 4, paddingVertical: 6 },
    placeName: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.ink },
    slash: { fontFamily: fonts.ui, fontSize: 13, color: colors.borderStrong },
    composer: { padding: 6, borderRadius: 10, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.composer },
    yoloComposer: { borderColor: 'transparent' },
    bar: { flexDirection: 'row', alignItems: 'center', gap: 2, paddingTop: 6 },
    add: { width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center' },
    picker: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 34, paddingHorizontal: 7 },
    pickerText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.accent },
    more: { width: 40, height: 40, borderRadius: 20, alignItems: 'center', justifyContent: 'center' },
    send: { width: 34, height: 34, borderRadius: 17, backgroundColor: colors.ink, alignItems: 'center', justifyContent: 'center' },

    agents: { flexDirection: 'row', flexWrap: 'wrap', gap: 6 },
    agent: {
      width: '23.6%',
      height: 66,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
      alignItems: 'center',
      justifyContent: 'center',
      gap: 6,
    },
    agentOn: { backgroundColor: colors.active, borderColor: colors.borderStrong },
    agentText: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },

    sheetHead: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingHorizontal: 4 },
    sheetTitle: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    group: { borderRadius: 14, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.raised, overflow: 'hidden' },
    option: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 52, paddingHorizontal: 14 },
    optionDivided: { borderTopWidth: 1, borderTopColor: colors.border },
    optionText: { flex: 1, fontFamily: fonts.uiMedium, fontSize: 15.5, color: colors.secondary },
  });
};
