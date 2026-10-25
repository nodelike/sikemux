import { useMemo, useRef, useState } from 'react';
import { KeyboardAvoidingView, Pressable, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { router, useLocalSearchParams } from 'expo-router';
import { File } from 'expo-file-system';
import type { ConnectionLike } from '@sikemux/native';

import { composerPlaceholder } from '@shared/chat/chatStatus';
import type { ChatMessage } from '@shared/chat/types';
import { agentLabel } from '@shared/agents/catalog';
import { ComposerAttachments } from '@/chat/Attachments';
import { ChatTitle } from '@/chat/ChatTitle';
import { Activity, Message } from '@/chat/Transcript';
import { chatSession } from '@/chat/useChat';
import { AttachSheet } from '@/chat/AttachSheet';
import { YoloToggle } from '@/chat/Composer';
import { ComposerInput } from '@/chat/ComposerInput';
import { fitting, uploadEach, type Attachment } from '@/chat/uploads';
import { MAX_ATTACHMENTS } from '@/core/wire.generated';
import type { LauncherInfo } from '@/core/protocol';
import { problem as problemOf } from '@/core/problem';
import { useLive } from '@/devices/hub';
import { ProjectSheet } from '@/devices/ProjectSheet';
import { haptics } from '@/ui/haptics';
import { AgentFace } from '@/ui/Buddy';
import { AgentIcon, Icon } from '@/ui/Icon';
import { type Glyph, PromptGlyph } from '@/ui/prompt';
import { Nav, Screen, useKeyboardShown } from '@/ui/screen';
import { Working } from '@/ui/status';
import { Sheet, SheetLabel } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles, useType } from '@/ui/theme';

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

export { Crashed as ErrorBoundary } from '@/screens/Crashed';

export default function NewChat() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const { core, project: linkedProject } = useLocalSearchParams<{ core: string; project?: string }>();
  const live = useLive(core);
  const workspace = live.snapshot?.workspace;
  const [launcherId, setLauncherId] = useState<string>();
  const [chosenMode, setChosenMode] = useState<string>();
  const [projectId, setProjectId] = useState<string | undefined>(linkedProject);
  const [draft, setDraft] = useState('');
  const [sheet, setSheet] = useState<'agent' | 'project' | 'attach' | 'settings'>();
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [sending, setSending] = useState<Sending>();
  const starting = Boolean(sending);
  const [problem, setProblem] = useState<string>();
  const [started, setStarted] = useState<string>();
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
  const room = MAX_ATTACHMENTS - attachments.length;
  const sendable = Boolean((draft.trim() || attachments.length) && launcher && project && live.status === 'open' && !starting);
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

  const attach = (picking: Promise<Attachment[]>) => {
    picking
      .then((picked) => {
        const { fits, notice } = fitting(attachments.length, picked);
        if (notice) setProblem(notice);
        setAttachments((now) => [...now, ...fits]);
      })
      .catch((error: unknown) => setProblem(`Could not pick: ${problemOf(error)}`));
  };

  /** Answers the files as they ended up, and all of them sent or null when one did not get there. */
  const upload = async (connection: ConnectionLike, agentId: string, picked: Attachment[]) => {
    let files = picked;
    const sent = await uploadEach(
      picked,
      async (attachment) => connection.attachFile(agentId, attachment.name, attachment.mime, await new File(attachment.uri).arrayBuffer()),
      (id, change) => {
        files = files.map((known) => (known.id === id ? { ...known, ...change } : known));
      },
      problemOf,
    );
    return { sent, files };
  };

  /** Puts what was sent back in the composer, to try again. */
  const giveBack = (shown: Sending, files: Attachment[], why: string) => {
    busy.current = false;
    haptics.failure();
    setProblem(why);
    setDraft(shown.draft);
    setAttachments(files);
    setSending(undefined);
  };

  const start = async () => {
    const text = draft.trim();
    if (busy.current || (!text && !attachments.length) || !launcher || !project || live.status !== 'open') return;
    busy.current = true;
    haptics.tap();
    const shown = sendingOf(draft, attachments);
    setSending(shown);
    setDraft('');
    setAttachments([]);
    setProblem(undefined);
    try {
      let agentId = started;
      if (!agentId) {
        agentId = await live.connection.startChat(launcher.id, project.id, chosenMode);
        setStarted(agentId);
      }
      const { sent, files } = await upload(live.connection, agentId, shown.attachments);
      if (!sent) return giveBack(shown, files, 'Could not send every file to the host');
      chatSession(core, agentId).sendWhenAttached(text, sent);
      router.replace({ pathname: '/device/[core]/chat/[agent]', params: { core, agent: agentId, started: '1' } });
    } catch (error) {
      giveBack(shown, shown.attachments, problemOf(error));
    }
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
            {launcher ? <Activity provider={launcher.provider} label="Starting" since={sending.since} /> : null}
          </View>
        ) : (
          <View style={styles.welcome}>
            {absent ? (
              <PromptGlyph icon={absent} small />
            ) : launcher ? (
              <AgentFace agentId={project?.id} provider={launcher.provider} size={38} logoSize={40} />
            ) : null}
            <Text style={styles.welcomeTitle}>{project?.name ?? 'New chat'}</Text>
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
          {project ? (
            <Pressable
              style={styles.strip}
              onPress={() => setSheet('project')}
              disabled={Boolean(started)}
              accessibilityRole="button"
              accessibilityLabel="Project">
              <Icon name="IconFolder" size={13} color={colors.live} />
              <Text style={styles.stripName}>{project.name}</Text>
              <View style={{ transform: [{ rotate: '90deg' }] }}>
                <Icon name="IconChevron" size={10} color={colors.inkFaint} />
              </View>
            </Pressable>
          ) : null}
          <View style={styles.composer}>
            {attachments.length ? (
              <ComposerAttachments
                attachments={attachments}
                onRemove={(id) => !starting && setAttachments((now) => now.filter((attachment) => attachment.id !== id))}
                onRetry={start}
              />
            ) : null}
            <ComposerInput value={draft} onChangeText={setDraft} placeholder={IDLE} editable={!starting} />
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
              {launcher ? (
                <YoloToggle mode={mode} provider={launcher.provider} locked={starting || Boolean(started)} onToggle={setChosenMode} />
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
                disabled={!sendable}
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
        </SafeAreaView>
      </KeyboardAvoidingView>
      <AttachSheet visible={sheet === 'attach'} room={room} onClose={() => setSheet(undefined)} onAttach={attach} />
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
      justifyContent: 'center',
      gap: 12,
      paddingHorizontal: 28,
    },
    sending: { flex: 1, justifyContent: 'flex-end', paddingHorizontal: 18, paddingBottom: 28 },
    welcomeTitle: { fontFamily: fonts.uiSemibold, fontSize: 20, letterSpacing: -0.55, color: colors.ink },
    problem: { fontFamily: fonts.ui, fontSize: 13.5, color: colors.danger, textAlign: 'center' },
    wrap: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 6 },
    strip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 7,
      marginHorizontal: 14,
      marginBottom: -1,
      paddingVertical: 9,
      paddingHorizontal: 12,
      borderWidth: 1,
      borderBottomWidth: 0,
      borderColor: colors.border,
      borderTopLeftRadius: 12,
      borderTopRightRadius: 12,
      backgroundColor: colors.composer,
    },
    stripName: { fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink },
    composer: { padding: 6, borderRadius: 16, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.composer },
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
