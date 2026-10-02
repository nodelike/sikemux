import { useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';

import type { ChatInfo, ProjectInfo, SessionInfo, Snapshot } from '@/core/protocol';
import { reloadDevices, retry, useDevices, useLive } from '@/devices/hub';
import { channelLabel, deviceKind, deviceName, updateDevice } from '@/devices/paired';
import { ForgetSheet } from '@/devices/ForgetSheet';
import { ProjectSheet } from '@/devices/ProjectSheet';
import { chatState, chatTitle, folder, providerName } from '@/devices/words';
import { AgentIcon, DeviceIcon, Icon } from '@/ui/Icon';
import { Button, Group, IconButton, Nav, NeedsYou, Row, Screen, SectionLabel, Track, useBottomGap, Working } from '@/ui/parts';
import { fonts, type Palette, radius, typeFor, useColors, useStyles, useType, translucent } from '@/ui/theme';

type Tab = 'agents' | 'terminals';

/** The New chat pill's height, which the list leaves room for below its last row. */
const NEW_CHAT_HEIGHT = 52;

function projectName(snapshot: Snapshot, cwd: string): string {
  return snapshot.workspace.projects.find((project) => project.path === cwd)?.name ?? folder(cwd);
}

function inProject(project: ProjectInfo, cwd: string): boolean {
  return cwd === project.path || cwd.startsWith(`${project.path}/`);
}

/** What runs in a project, as its row in the project sheet shows it. */
function ProjectTail({ snapshot, project }: { snapshot: Snapshot; project: ProjectInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const chats = snapshot.chats.filter((chat) => inProject(project, chat.cwd));
  const terminals = snapshot.sessions.filter((session) => session.project === project.id && session.running).length;
  const waiting = chats.some((chat) => chat.pendingPermissions.length);
  if (!chats.length && !terminals) return null;
  return (
    <View style={styles.tail}>
      {waiting ? <NeedsYou /> : null}
      {chats.length ? (
        <View style={styles.faces}>
          {chats.slice(0, 3).map((chat, index) => (
            <View key={chat.agentId} style={[styles.face, index > 0 && { marginLeft: -7 }]}>
              <AgentIcon provider={chat.provider} size={14} />
            </View>
          ))}
        </View>
      ) : (
        <View style={styles.termCount}>
          <Icon name="IconCommand" size={13} color={colors.tertiary} />
          <Text style={type.meta}>{terminals}</Text>
        </View>
      )}
    </View>
  );
}

function ChatEnd({ chat }: { chat: ChatInfo }) {
  if (chat.pendingPermissions.length) return <NeedsYou />;
  if (chat.running) return <Working />;
  return null;
}

function Agents({ core, snapshot, scope }: { core: string; snapshot: Snapshot; scope?: ProjectInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const open = (agentId: string) => router.push(`/device/${core}/chat/${agentId}`);
  const [filter, setFilter] = useState<string>('all');
  const scoped = snapshot.chats.filter((chat) => !scope || inProject(scope, chat.cwd));
  const providers = [...new Set(scoped.map((chat) => chat.provider))];
  const shown = providers.includes(filter) ? filter : 'all';
  const chats = scoped.filter((chat) => shown === 'all' || chat.provider === shown);
  const where = (chat: ChatInfo, state: string) => (scope ? state : `${projectName(snapshot, chat.cwd)} · ${state}`);
  const asking = chats.filter((chat) => chat.pendingPermissions.length);
  const idle = chats.filter((chat) => !chat.pendingPermissions.length);

  return (
    <>
      {providers.length > 1 ? (
        <View style={styles.filters}>
          <Pressable
            onPress={() => setFilter('all')}
            style={[styles.filter, shown === 'all' && styles.filterOn]}
            accessibilityRole="button"
            accessibilityState={{ selected: shown === 'all' }}>
            <Text style={[styles.filterText, shown === 'all' && { color: colors.ink }]}>All</Text>
          </Pressable>
          {providers.map((provider) => (
            <Pressable
              key={provider}
              onPress={() => setFilter(provider)}
              style={[styles.filter, shown === provider && styles.filterOn]}
              accessibilityRole="button"
              accessibilityLabel={providerName(provider)}
              accessibilityState={{ selected: shown === provider }}>
              <AgentIcon provider={provider} size={17} />
            </Pressable>
          ))}
        </View>
      ) : null}
      {asking.map((chat) => (
        <Pressable key={chat.agentId} style={styles.ask} onPress={() => open(chat.agentId)} accessibilityRole="button">
          <AgentIcon provider={chat.provider} size={22} />
          <View style={{ flex: 1 }}>
            <Text style={styles.askTitle} numberOfLines={1}>
              {chatTitle(chat)}
            </Text>
            <Text style={styles.askDetail} numberOfLines={1}>
              {where(chat, 'Needs input')}
            </Text>
          </View>
          <NeedsYou />
        </Pressable>
      ))}
      {idle.length ? (
        <>
          <SectionLabel>Open</SectionLabel>
          <Group>
            {idle.map((chat) => (
              <Row
                key={chat.agentId}
                mark={<AgentIcon provider={chat.provider} size={22} />}
                title={chatTitle(chat)}
                detail={where(chat, chatState(chat))}
                end={<ChatEnd chat={chat} />}
                onPress={() => open(chat.agentId)}
              />
            ))}
          </Group>
        </>
      ) : null}
      {!scoped.length ? <Text style={styles.empty}>{scope ? `No agents in ${scope.name}.` : 'No agents running.'}</Text> : null}
    </>
  );
}

function terminalTitle(session: SessionInfo): string {
  if (session.task) return session.task.label;
  return session.agentType ?? 'Terminal';
}

function terminalDetail(session: SessionInfo): string {
  if (session.task) return session.task.command;
  if (session.running) return 'Running';
  if (session.exit?.signal) return `stopped by ${session.exit.signal}`;
  return session.exit?.code != null ? `exited ${session.exit.code}` : 'exited';
}

function Terminals({ snapshot, scope }: { snapshot: Snapshot; scope?: ProjectInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const groups = new Map<string, SessionInfo[]>();
  for (const session of snapshot.sessions) {
    if (scope && session.project !== scope.id) continue;
    const project = snapshot.workspace.projects.find((known) => known.id === session.project)?.name ?? 'Other';
    groups.set(project, [...(groups.get(project) ?? []), session]);
  }
  if (!groups.size) return <Text style={styles.empty}>{scope ? `No terminals in ${scope.name}.` : 'No terminals open.'}</Text>;
  return (
    <>
      {[...groups].map(([project, sessions]) => (
        <View key={project}>
          {scope ? <View style={{ height: 14 }} /> : <SectionLabel>{project}</SectionLabel>}
          <Group>
            {sessions.map((session) => (
              <Row
                key={session.id}
                dim={!session.running}
                mark={
                  <Icon
                    name={session.task ? 'IconRun' : 'IconCommand'}
                    size={session.task ? 15 : 18}
                    color={session.running ? colors.secondary : colors.tertiary}
                  />
                }
                title={terminalTitle(session)}
                detail={<Text style={type.mono}>{terminalDetail(session)}</Text>}
                end={session.running ? session.task ? <Working /> : <View style={styles.liveDot} /> : null}
              />
            ))}
          </Group>
        </View>
      ))}
    </>
  );
}

function summary(snapshot: Snapshot): string {
  const terminals = snapshot.sessions.filter((session) => session.running).length;
  return `${snapshot.chats.length} agent${snapshot.chats.length === 1 ? '' : 's'} · ${terminals} terminal${terminals === 1 ? '' : 's'}`;
}

export default function Device() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const { core, tab: linkedTab } = useLocalSearchParams<{ core: string; tab?: Tab }>();
  const { devices } = useDevices();
  const device = devices.find((known) => known.core === core);
  const live = useLive(core);
  const [tab, setTab] = useState<Tab>(linkedTab === 'terminals' ? 'terminals' : 'agents');
  const [followedLink, setFollowedLink] = useState(linkedTab);
  if (linkedTab !== followedLink) {
    setFollowedLink(linkedTab);
    if (linkedTab === 'agents' || linkedTab === 'terminals') setTab(linkedTab);
  }
  const away = live.status === 'closed';
  const channel = channelLabel(device?.channel);
  const behind = live.status === 'closed' ? live.outdated : undefined;
  const unreachable =
    behind === 'mac'
      ? { title: 'This Mac needs a newer Sikemux', body: 'Update Sikemux on the Mac, then come back here.' }
      : behind === 'phone'
        ? { title: 'Update this app', body: 'This Mac runs a newer Sikemux than this app understands.' }
        : { title: "Can't reach this Mac", body: 'It may be asleep, offline, or have remote access turned off.' };
  const snapshot = live.snapshot;
  const [picking, setPicking] = useState(false);
  const [options, setOptions] = useState(false);
  const starts = device?.access === 'full' && !away && tab === 'agents';
  const bottom = useBottomGap();
  const scope = snapshot?.workspace.projects.find((project) => project.id === device?.project);
  const asking = snapshot?.chats.filter((chat) => chat.pendingPermissions.length && (!scope || inProject(scope, chat.cwd))).length ?? 0;
  const scopeTo = (project: string | null) => {
    setPicking(false);
    updateDevice(core, { project: project ?? undefined }).then(reloadDevices);
  };

  return (
    <Screen>
      <Nav back="Devices" end={device ? <IconButton name="IconMore" label="Options" onPress={() => setOptions(true)} /> : null} />
      {device ? <ForgetSheet device={device} visible={options} onClose={() => setOptions(false)} /> : null}
      <View style={styles.header}>
        <View style={styles.glyph}>
          <DeviceIcon kind={deviceKind(device?.model)} color={away ? colors.tertiary : colors.ink} />
          <View style={[styles.presence, away ? styles.presenceOff : styles.presenceOn]} />
        </View>
        <View style={{ flex: 1 }}>
          <Text style={styles.name} numberOfLines={1}>
            {device ? deviceName(device) : 'Mac'}
          </Text>
          <Text style={type.meta} numberOfLines={1}>
            {[channel, behind ? 'Needs an update' : away ? 'Asleep or offline' : snapshot ? summary(snapshot) : null]
              .filter(Boolean)
              .join(' · ')}
          </Text>
        </View>
      </View>
      {away && !snapshot ? (
        <View style={styles.away}>
          <Text style={[type.title, { fontSize: 20, textAlign: 'center' }]}>{unreachable.title}</Text>
          <Text style={[type.body, { textAlign: 'center', marginTop: 8 }]}>{unreachable.body}</Text>
          {behind ? null : (
            <>
              <View style={styles.trying}>
                <Working />
                <Text style={type.meta}>Trying again</Text>
              </View>
              <Button title="Try now" onPress={() => retry(core)} style={styles.retry} />
            </>
          )}
        </View>
      ) : !snapshot ? (
        <View style={styles.away}>
          <View style={styles.trying}>
            <Working />
            <Text style={type.meta}>Connecting…</Text>
          </View>
        </View>
      ) : (
        <>
          {snapshot?.workspace.projects.length ? (
            <View style={styles.scope}>
              <Pressable onPress={() => setPicking(true)} style={styles.pill} accessibilityRole="button" accessibilityLabel="Project">
                <Icon name="IconFolder" size={14} color={colors.live} />
                <Text style={styles.pillText} numberOfLines={1}>
                  {scope?.name ?? 'All projects'}
                </Text>
                <View style={{ transform: [{ rotate: '90deg' }] }}>
                  <Icon name="IconChevron" size={10} color={colors.inkFaint} />
                </View>
              </Pressable>
            </View>
          ) : null}
          <View style={{ paddingHorizontal: 16 }}>
            <Track<Tab>
              value={tab}
              onChange={setTab}
              options={[
                { value: 'agents', label: 'Agents', count: asking },
                { value: 'terminals', label: 'Terminals' },
              ]}
            />
          </View>
          <ScrollView contentContainerStyle={[styles.body, { paddingBottom: bottom + (starts ? NEW_CHAT_HEIGHT + 24 : 24) }]}>
            {snapshot ? (
              tab === 'agents' ? (
                <Agents core={core} snapshot={snapshot} scope={scope} />
              ) : (
                <Terminals snapshot={snapshot} scope={scope} />
              )
            ) : null}
          </ScrollView>
          {starts ? (
            <Pressable
              onPress={() => router.push(scope ? `/device/${core}/new?project=${encodeURIComponent(scope.id)}` : `/device/${core}/new`)}
              style={({ pressed }) => [styles.newChat, { bottom }, pressed && { opacity: 0.85 }]}
              accessibilityRole="button"
              accessibilityLabel="New chat">
              <Icon name="IconPlus" size={18} color={colors.ground} />
              <Text style={styles.newChatText}>New chat</Text>
            </Pressable>
          ) : null}
          {snapshot ? (
            <ProjectSheet
              visible={picking}
              onClose={() => setPicking(false)}
              device={device ? deviceName(device) : undefined}
              projects={snapshot.workspace.projects}
              chosen={scope?.id ?? null}
              onChoose={scopeTo}
              all={summary(snapshot)}
              tail={(project) => <ProjectTail snapshot={snapshot} project={project} />}
            />
          ) : null}
        </>
      )}
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    header: { flexDirection: 'row', alignItems: 'center', gap: 14, paddingHorizontal: 18, paddingTop: 4, paddingBottom: 16 },
    glyph: { width: 34, height: 34, alignItems: 'center', justifyContent: 'center' },
    presence: { position: 'absolute', right: -1, bottom: 3, width: 10, height: 10, borderRadius: 5, borderWidth: 2.5 },
    presenceOn: { backgroundColor: colors.live, borderColor: colors.ground },
    presenceOff: { backgroundColor: colors.ground, borderColor: colors.rest },
    name: { ...type.title },
    body: { paddingHorizontal: 16 },
    newChat: {
      position: 'absolute',
      right: 16,
      height: NEW_CHAT_HEIGHT,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      paddingLeft: 16,
      paddingRight: 20,
      borderRadius: NEW_CHAT_HEIGHT / 2,
      backgroundColor: colors.ink,
      shadowColor: '#000',
      shadowOpacity: 0.45,
      shadowRadius: 18,
      shadowOffset: { width: 0, height: 8 },
      elevation: 8,
    },
    newChatText: { fontFamily: fonts.uiSemibold, fontSize: 15, color: colors.ground },
    scope: { flexDirection: 'row', paddingHorizontal: 16, paddingBottom: 12 },
    pill: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 7,
      height: 32,
      maxWidth: '100%',
      paddingHorizontal: 11,
      borderRadius: 16,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    pillText: { fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink, flexShrink: 1 },
    tail: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    faces: { flexDirection: 'row' },
    face: {
      width: 26,
      height: 26,
      borderRadius: 13,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
      alignItems: 'center',
      justifyContent: 'center',
    },
    termCount: { flexDirection: 'row', alignItems: 'center', gap: 4 },
    filters: { flexDirection: 'row', gap: 6, paddingTop: 12 },
    filter: {
      minHeight: 32,
      minWidth: 40,
      paddingHorizontal: 12,
      borderRadius: 16,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: translucent(colors.raised, 0.8),
      alignItems: 'center',
      justifyContent: 'center',
    },
    filterOn: { backgroundColor: translucent(colors.overlay, 0.9), borderColor: colors.borderStrong },
    filterText: { fontFamily: fonts.ui, fontSize: 13, color: colors.secondary },
    ask: {
      marginTop: 14,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      padding: 14,
      borderRadius: radius.card,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      backgroundColor: colors.overlay,
    },
    askTitle: { fontFamily: fonts.uiMedium, fontSize: 15.5, color: colors.ink },
    askDetail: { ...type.meta, marginTop: 2 },
    empty: { ...type.meta, textAlign: 'center', paddingTop: 40 },
    liveDot: { width: 7, height: 7, borderRadius: 4, backgroundColor: colors.live },
    away: { flex: 1, justifyContent: 'center', paddingHorizontal: 32, paddingBottom: 120 },
    trying: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 10, marginTop: 16 },
    retry: { marginTop: 24 },
  });
};
