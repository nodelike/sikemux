import { useMemo, useState } from 'react';
import { KeyboardAvoidingView, Pressable, StyleSheet, Text, View } from 'react-native';
import { SafeAreaView } from 'react-native-safe-area-context';
import { router, useLocalSearchParams } from 'expo-router';
import * as Haptics from 'expo-haptics';

import { composerPlaceholder } from '@mac/chat/chatStatus';
import { ComposerInput } from '@/chat/ComposerInput';
import type { LauncherInfo } from '@/core/protocol';
import { problem as problemOf, useLive } from '@/devices/hub';
import { ProjectSheet } from '@/devices/ProjectSheet';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Nav, Screen, useKeyboardShown, Working } from '@/ui/parts';
import { Sheet, SheetLabel } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles, useType } from '@/ui/theme';

const IDLE = composerPlaceholder({ connection: 'ready', running: false }, { resuming: false, disconnected: false });

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
  return (
    <Sheet visible={visible} onClose={onClose}>
      <SheetLabel>Agent</SheetLabel>
      <View style={styles.agents}>
        {launchers.map((launcher) => {
          const on = launcher.id === chosen?.id;
          return (
            <Pressable
              key={launcher.id}
              onPress={() => onChoose(launcher)}
              style={[styles.agent, on && styles.agentOn]}
              accessibilityRole="button">
              <AgentIcon provider={launcher.provider} size={22} />
              <Text style={[styles.agentText, on && { color: colors.ink }]} numberOfLines={1}>
                {launcher.label}
              </Text>
            </Pressable>
          );
        })}
      </View>
    </Sheet>
  );
}

export default function NewChat() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const { core, project: linkedProject } = useLocalSearchParams<{ core: string; project?: string }>();
  const live = useLive(core);
  const workspace = live.snapshot?.workspace;
  const [launcherId, setLauncherId] = useState<string>();
  const [projectId, setProjectId] = useState<string | undefined>(linkedProject);
  const [draft, setDraft] = useState('');
  const [sheet, setSheet] = useState<'agent' | 'project'>();
  const [starting, setStarting] = useState(false);
  const [problem, setProblem] = useState<string>();
  const [started, setStarted] = useState<string>();
  const typing = useKeyboardShown();
  const launcher = useMemo(
    () => workspace?.launchers.find((known) => known.id === launcherId) ?? workspace?.launchers[0],
    [workspace, launcherId],
  );
  const project = useMemo(
    () => workspace?.projects.find((known) => known.id === projectId) ?? workspace?.projects[0],
    [workspace, projectId],
  );
  const yolo = launcher?.permissionMode === 'bypass' || launcher?.permissionMode === 'bypassPermissions';
  const sendable = Boolean(draft.trim() && launcher && project && live.status === 'open' && !starting);

  const start = async () => {
    const text = draft.trim();
    if (!text || !launcher || !project || live.status !== 'open') return;
    Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    setStarting(true);
    setProblem(undefined);
    try {
      let agentId = started;
      if (!agentId) {
        agentId = await live.connection.startChat(launcher.id, project.id);
        setStarted(agentId);
      }
      await live.connection.prompt(agentId, text);
      router.replace(`/device/${core}/chat/${agentId}`);
    } catch (error) {
      setProblem(problemOf(error));
      setStarting(false);
    }
  };

  return (
    <Screen>
      <Nav back="Cancel" title="New chat" />
      <KeyboardAvoidingView style={{ flex: 1 }} behavior="padding">
        <View style={styles.welcome}>
          {launcher ? <AgentIcon provider={launcher.provider} size={40} /> : null}
          <Text style={styles.welcomeTitle}>{project?.name ?? 'New chat'}</Text>
          {workspace && !workspace.launchers.length ? (
            <Text style={[type.meta, { textAlign: 'center' }]}>Open Sikemux on the Mac so it can offer its agents.</Text>
          ) : null}
          {problem ? <Text style={styles.problem}>{problem}</Text> : null}
        </View>
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
            <ComposerInput value={draft} onChangeText={setDraft} placeholder={IDLE} editable={!starting} />
            <View style={styles.bar}>
              <View style={styles.yolo}>
                <Icon name={yolo ? 'IconShieldBolt' : 'IconShield'} size={13} color={yolo ? colors.accent : colors.inkFaint} />
                <Text style={[styles.yoloText, yolo && { color: colors.accent }]}>{yolo ? 'yolo' : 'safe'}</Text>
              </View>
              {launcher ? (
                <Pressable
                  style={styles.picker}
                  onPress={() => setSheet('agent')}
                  disabled={Boolean(started)}
                  accessibilityRole="button"
                  accessibilityLabel="Agent">
                  <AgentIcon provider={launcher.provider} size={16} />
                  <Text style={styles.pickerText}>{launcher.label}</Text>
                  <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
                    <Icon name="IconChevron" size={10} color={colors.accent} />
                  </View>
                </Pressable>
              ) : null}
              <View style={{ flex: 1 }} />
              <Pressable
                onPress={start}
                disabled={!sendable}
                style={[styles.send, !sendable && { opacity: 0.28 }]}
                accessibilityRole="button"
                accessibilityLabel="Start"
                accessibilityState={{ disabled: !sendable }}>
                {starting ? <Working /> : <Icon name="IconArrowUp" size={16} color={colors.ground} />}
              </Pressable>
            </View>
          </View>
        </SafeAreaView>
      </KeyboardAvoidingView>
      <AgentSheet
        visible={sheet === 'agent'}
        onClose={() => setSheet(undefined)}
        launchers={workspace?.launchers ?? []}
        chosen={launcher}
        onChoose={(next) => {
          setLauncherId(next.id);
          setSheet(undefined);
        }}
      />
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
      borderTopWidth: 1,
      borderTopColor: colors.border,
    },
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
    yolo: { flexDirection: 'row', alignItems: 'center', gap: 4, height: 34, paddingHorizontal: 8 },
    yoloText: { fontFamily: fonts.uiSemibold, fontSize: 11, letterSpacing: 0.9, textTransform: 'uppercase', color: colors.inkFaint },
    picker: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 34, paddingHorizontal: 7 },
    pickerText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.accent },
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
  });
};
