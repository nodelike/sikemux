import { Fragment, memo, useEffect, useRef, useState } from 'react';
import { Animated, Pressable, StyleSheet, Text, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import MaskedView from '@react-native-masked-view/masked-view';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { agentLabel } from '@shared/agents/catalog';
import { formatAge } from '@shared/lib/format';
import type { ChatInfo, ProjectInfo, RecentInfo, SessionInfo, Snapshot } from '@/core/protocol';
import type { AgentShown } from '@/core/wire.generated';
import { problem as problemOf } from '@/core/problem';
import { connectionOf, reloadDevices, retry, useDevices, useHostStatus, useLive } from '@/devices/hub';
import { deviceKind, deviceName, type PairedDevice, updateDevice } from '@/devices/paired';
import { Board } from '@/devices/Board';
import { ForgetSheet, useForget } from '@/devices/ForgetSheet';
import { ProjectSheet } from '@/devices/ProjectSheet';
import { WorkerRow, workersOf } from '@/devices/RemoteWorker';
import { summary } from '@/devices/status';
import { agentIn, itemIn, projectName, projectOf } from '@/devices/words';
import { haptics } from '@/ui/haptics';
import { useMinute, useScrollPause } from '@/ui/motion';
import { AgentFace } from '@/ui/Buddy';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Button, IconButton } from '@/ui/controls';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { EmptyState, Prompt } from '@/ui/prompt';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { AgentState, NeedsYou, ShellCount, SubagentCount, Working } from '@/ui/status';
import { fonts, isLight, type Palette, typeFor, useColors, useStyles, useType, translucent } from '@/ui/theme';

type Tab = 'agents' | 'terminals' | 'board';

/** The New chat pill's height, which the list leaves room for below its last row. */
const NEW_CHAT_HEIGHT = 52;
/** Recent chats shown before Show more. */
const RECENT_SHOWN = 10;

/** What runs in a project, as its row in the project sheet shows it. */
function ProjectTail({ snapshot, project }: { snapshot: Snapshot; project: ProjectInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const chats = snapshot.chats.filter((chat) => agentIn(snapshot, project, chat.agentId, chat.cwd));
  const terminals = snapshot.sessions.filter((session) => session.project === project.id && session.running).length;
  const waiting = snapshot.inbox.items.some((item) => item.tier === 'needs' && itemIn(snapshot, item, project));
  if (!chats.length && !terminals) return null;
  return (
    <View style={styles.tail}>
      {waiting ? <NeedsYou /> : null}
      {chats.length ? (
        <View style={styles.faces}>
          {chats.slice(0, 3).map((chat, index) => (
            <View key={chat.agentId} style={[styles.face, index > 0 && { marginLeft: -4 }]}>
              <AgentFace agentId={chat.agentId} provider={chat.provider} group={chat.group} size={16} />
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

/** Saved chats the host's rail lists as recent; a phone with full access takes one up again. */
function Recent({
  core,
  snapshot,
  scope,
  provider,
  full,
  hostName,
}: {
  core: string;
  snapshot: Snapshot;
  scope?: ProjectInfo;
  provider: string;
  full: boolean;
  hostName: string;
}) {
  const styles = useStyles(makeStyles);
  const live = useLive(core);
  const minute = useMinute();
  const [all, setAll] = useState(false);
  const [resuming, setResuming] = useState<string>();
  const [note, setNote] = useState<{ text: string; failed: boolean }>();
  const chats = snapshot.recent.filter(
    (chat) => !chat.agentId && (!scope || chat.project === scope.id) && (provider === 'all' || chat.provider === provider),
  );
  const shown = all ? chats : chats.slice(0, RECENT_SHOWN);

  const resume = async (chat: RecentInfo) => {
    if (resuming) return;
    if (!full) {
      haptics.warning();
      setNote({ text: `This phone can only watch ${hostName}.`, failed: false });
      return;
    }
    if (live.status !== 'open') {
      haptics.failure();
      setNote({ text: 'Not connected to the host.', failed: true });
      return;
    }
    haptics.tap();
    setNote(undefined);
    setResuming(chat.id);
    try {
      const agentId = await live.connection.resumeChat(chat.id);
      router.push(`/device/${core}/chat/${agentId}`);
    } catch (error) {
      haptics.failure();
      setNote({ text: problemOf(error), failed: true });
    } finally {
      setResuming(undefined);
    }
  };

  // The rows only redraw when their own chat does, so they call the latest resume through a ref.
  const latestResume = useRef(resume);
  useEffect(() => {
    latestResume.current = resume;
  });
  const onResume = (chat: RecentInfo) => void latestResume.current(chat);

  if (!chats.length) return null;
  return (
    <>
      <SectionLabel>Recent</SectionLabel>
      <Rows>
        {shown.map((chat) => (
          <RecentRow
            key={chat.id}
            chat={chat}
            place={scope ? undefined : projectName(snapshot, chat.project, chat.cwd)}
            resuming={resuming === chat.id}
            minute={minute}
            onResume={onResume}
          />
        ))}
        {shown.length < chats.length ? (
          <Pressable
            onPress={() => setAll(true)}
            style={({ pressed }) => [styles.more, pressed && styles.morePressed]}
            accessibilityRole="button">
            <Text style={styles.moreText}>Show {chats.length - shown.length} more</Text>
          </Pressable>
        ) : null}
      </Rows>
      {note ? (
        <Text style={[styles.note, note.failed && styles.noteFailed]} accessibilityLiveRegion="polite">
          {note.text}
        </Text>
      ) : null}
    </>
  );
}

type OpenAgent = { shown: AgentShown } & ({ kind: 'chat'; chat: ChatInfo } | { kind: 'terminal'; session: SessionInfo });

/** The open agents in the order the host lists them on every device, what wants the person first. */
function openAgents(snapshot: Snapshot, scope: ProjectInfo | undefined, provider: string): OpenAgent[] {
  return snapshot.inbox.agents.flatMap((shown): OpenAgent[] => {
    const chat = snapshot.chats.find((candidate) => candidate.agentId === shown.agentId);
    if (chat) {
      const listed = (!scope || agentIn(snapshot, scope, chat.agentId, chat.cwd)) && (provider === 'all' || chat.provider === provider);
      return listed ? [{ kind: 'chat', chat, shown }] : [];
    }
    const session = snapshot.sessions.find((candidate) => candidate.agentId === shown.agentId);
    const listed = session && (!scope || session.project === scope.id) && (provider === 'all' || session.agentType === provider);
    return session && listed ? [{ kind: 'terminal', session, shown }] : [];
  });
}

/** The host's agents: its open agents in one list, each with its own state, then its recent chats. */
function Agents({
  core,
  snapshot,
  scope,
  provider,
  full,
  hostName,
  onTab,
}: {
  core: string;
  snapshot: Snapshot;
  scope?: ProjectInfo;
  provider: string;
  full: boolean;
  hostName: string;
  onTab: (tab: Tab) => void;
}) {
  const styles = useStyles(makeStyles);
  const open = openAgents(snapshot, scope, provider);
  const workers = snapshot.inbox.remoteAgents;
  const leads = new Set(open.map((agent) => (agent.kind === 'chat' ? agent.chat.agentId : agent.session.agentId)));
  const strays = scope
    ? []
    : workers.filter((worker) => !(worker.lead && leads.has(worker.lead)) && (provider === 'all' || worker.cli === provider));
  const placeOf = (project: string | undefined) => (scope || !project ? undefined : projectName(snapshot, project));

  return (
    <>
      {open.length || strays.length ? (
        <Rows style={styles.open}>
          {open.map((agent) =>
            agent.kind === 'chat' ? (
              <Fragment key={agent.chat.agentId}>
                <ChatRow
                  core={core}
                  chat={agent.chat}
                  shown={agent.shown}
                  place={placeOf(projectOf(snapshot, agent.chat.agentId) ?? agent.chat.cwd)}
                />
                {workersOf(workers, agent.chat.agentId).map((worker) => (
                  <WorkerRow key={worker.agentId} worker={worker} nested />
                ))}
              </Fragment>
            ) : (
              <Fragment key={agent.session.id}>
                <TerminalAgentRow
                  session={agent.session}
                  shown={agent.shown}
                  place={placeOf(agent.session.project)}
                  onOpen={() => onTab('terminals')}
                />
                {agent.session.agentId
                  ? workersOf(workers, agent.session.agentId).map((worker) => <WorkerRow key={worker.agentId} worker={worker} nested />)
                  : null}
              </Fragment>
            ),
          )}
          {strays.map((worker) => (
            <WorkerRow key={worker.agentId} worker={worker} />
          ))}
        </Rows>
      ) : null}
      <Recent core={core} snapshot={snapshot} scope={scope} provider={provider} full={full} hostName={hostName} />
      {!open.length && !strays.length && !snapshot.recent.some((chat) => !scope || chat.project === scope.id) ? (
        <EmptyState icon="IconAgent" text={scope ? `No agents in ${scope.name}.` : 'No agents running.'} />
      ) : null}
    </>
  );
}

function terminalTitle(session: SessionInfo): string {
  if (session.task) return session.task.label;
  if (session.agentName) return session.title ?? agentLabel(session.agentType ?? 'agent');
  return session.agentType ?? 'Terminal';
}

function terminalDetail(session: SessionInfo): string {
  if (session.task) return session.task.command;
  if (session.running) return 'Running';
  if (session.exit?.signal) return `stopped by ${session.exit.signal}`;
  return session.exit?.code != null ? `exited ${session.exit.code}` : 'exited';
}

/* A host sends its whole view several times a second while an agent works; each row redraws only when its own
   item changed, which the hub keeps as the same object otherwise. */
const ChatRow = memo(function ChatRow({ core, chat, shown, place }: { core: string; chat: ChatInfo; shown: AgentShown; place?: string }) {
  return (
    <Row
      mark={<AgentFace agentId={chat.agentId} provider={chat.provider} group={shown.group} size={20} />}
      name={chat.name}
      title={shown.title}
      place={place}
      end={
        <>
          {shown.subagents ? <SubagentCount count={shown.subagents} /> : null}
          {shown.tasks ? <ShellCount count={shown.tasks} /> : null}
          <AgentState group={shown.group} />
        </>
      }
      onPress={() => router.push(`/device/${core}/chat/${chat.agentId}`)}
    />
  );
});

/** An agent running in a terminal, listed with the chats as the host's rail lists it. */
const TerminalAgentRow = memo(function TerminalAgentRow({
  session,
  shown,
  place,
  onOpen,
}: {
  session: SessionInfo;
  shown: AgentShown;
  place?: string;
  onOpen: () => void;
}) {
  const { group } = shown;
  return (
    <Row
      mark={<AgentFace agentId={session.agentId} provider={session.agentType ?? 'agent'} group={group} size={20} />}
      name={session.agentName}
      title={shown.title}
      place={place}
      end={<AgentState group={group} />}
      onPress={onOpen}
    />
  );
});

const RecentRow = memo(function RecentRow({
  chat,
  place,
  resuming,
  minute,
  onResume,
}: {
  chat: RecentInfo;
  place?: string;
  resuming: boolean;
  /** Ages the "5m" label once a minute. */
  minute: number;
  onResume: (chat: RecentInfo) => void;
}) {
  const styles = useStyles(makeStyles);
  const type = useType();
  return (
    <Row
      mark={<AgentIcon provider={chat.provider} size={20} />}
      title={chat.title}
      place={place}
      end={resuming ? <Working /> : <Text style={[type.meta, styles.age]}>{formatAge(Number(chat.activeAt), minute * 60_000)}</Text>}
      onPress={() => onResume(chat)}
    />
  );
});

const TerminalRow = memo(function TerminalRow({ session }: { session: SessionInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  return (
    <Row
      dim={!session.running}
      mark={
        <Icon
          name={session.task ? 'IconRun' : 'IconCommand'}
          size={session.task ? 15 : 18}
          color={session.running ? colors.secondary : colors.tertiary}
        />
      }
      name={session.task ? undefined : session.agentName}
      title={terminalTitle(session)}
      detail={<Text style={type.mono}>{terminalDetail(session)}</Text>}
      end={session.running ? session.task ? <Working /> : <View style={styles.liveDot} /> : null}
    />
  );
});

function Terminals({ snapshot, scope }: { snapshot: Snapshot; scope?: ProjectInfo }) {
  const groups = new Map<string, SessionInfo[]>();
  for (const session of snapshot.sessions) {
    if (scope && session.project !== scope.id) continue;
    const project = snapshot.workspace.projects.find((known) => known.id === session.project)?.name ?? 'Other';
    groups.set(project, [...(groups.get(project) ?? []), session]);
  }
  if (!groups.size) return <EmptyState icon="IconCommand" text={scope ? `No terminals in ${scope.name}.` : 'No terminals open.'} />;
  return (
    <>
      {[...groups].map(([project, sessions]) => (
        <View key={project}>
          {scope ? <View style={{ height: 8 }} /> : <SectionLabel>{project}</SectionLabel>}
          <Rows>
            {sessions.map((session) => (
              <TerminalRow key={session.id} session={session} />
            ))}
          </Rows>
        </View>
      ))}
    </>
  );
}

/** The host's name set large over its backdrop, under a line saying whether it is online and what runs on it. */
function HostHead({ name, line, online }: { name: string; line: string; online: boolean }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.head}>
      <View style={styles.where}>
        <View style={online ? styles.online : styles.offline} />
        <Text style={styles.whereText} numberOfLines={1}>
          {line}
        </Text>
      </View>
      <Text style={styles.name} numberOfLines={1} maxFontSizeMultiplier={1.4} accessibilityRole="header">
        {name}
      </Text>
    </View>
  );
}

/** How far rows take to fade out as they reach the tabs pinned at the top. */
const FADE = 22;

/**
 * The list's mask: once the tabs are pinned, nothing shows under them and rows fade in just below.
 * Until then `open` keeps the whole list shown, so the header is never cut. Rows also fade out into the home bar.
 */
function UnderBar({ height, open, bottom }: { height: number; open: Animated.AnimatedInterpolation<number>; bottom: number }) {
  return (
    <View style={{ flex: 1 }}>
      <View style={{ height }} />
      <Svg width="100%" height={FADE}>
        <Defs>
          <LinearGradient id="under-bar" x1="0" y1="0" x2="0" y2="1">
            <Stop offset="0" stopColor="#000" stopOpacity={0} />
            <Stop offset="1" stopColor="#000" stopOpacity={1} />
          </LinearGradient>
        </Defs>
        <Rect x="0" y="0" width="100%" height="100%" fill="url(#under-bar)" />
      </Svg>
      <View style={{ flex: 1, backgroundColor: '#000' }} />
      <Svg width="100%" height={bottom}>
        <Defs>
          <LinearGradient id="over-home" x1="0" y1="0" x2="0" y2="1">
            <Stop offset="0" stopColor="#000" stopOpacity={1} />
            <Stop offset="1" stopColor="#000" stopOpacity={0} />
          </LinearGradient>
        </Defs>
        <Rect x="0" y="0" width="100%" height="100%" fill="url(#over-home)" />
      </Svg>
      <Animated.View style={{ position: 'absolute', top: 0, left: 0, right: 0, bottom, backgroundColor: '#000', opacity: open }} />
    </View>
  );
}

function HostTab({ label, count, on, onPress }: { label: string; count?: number; on: boolean; onPress: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable onPress={onPress} hitSlop={12} style={styles.tab} accessibilityRole="tab" accessibilityState={{ selected: on }}>
      <Text style={[styles.tabText, on && { color: colors.ink }]}>{label}</Text>
      {count ? (
        <View style={styles.count}>
          <Text style={styles.countText} maxFontSizeMultiplier={1.3}>
            {count}
          </Text>
        </View>
      ) : null}
    </Pressable>
  );
}

/** The host turned this phone away: only forgetting it here and connecting again helps. */
function Unpaired({ device }: { device?: PairedDevice }) {
  const styles = useStyles(makeStyles);
  const { forgetting, problem, leave } = useForget(device);
  return (
    <Prompt icon="IconUnlink" title="This host no longer knows this phone" fill>
      {problem ? <Text style={styles.problem}>{problem}</Text> : null}
      <Button
        kind="danger"
        icon="IconTrash"
        title={forgetting ? 'Forgetting…' : 'Forget this host'}
        onPress={() => void leave()}
        disabled={forgetting || !device}
        style={styles.retry}
      />
    </Prompt>
  );
}

export { Crashed as ErrorBoundary } from '@/screens/Crashed';

export default function Device() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const { core, tab: linkedTab } = useLocalSearchParams<{ core: string; tab?: Tab }>();
  const { devices } = useDevices();
  const device = devices.find((known) => known.core === core);
  const live = useLive(core);
  const status = useHostStatus(core);
  const scrollPause = useScrollPause();
  const [tab, setTab] = useState<Tab>(linkedTab === 'terminals' || linkedTab === 'board' ? linkedTab : 'agents');
  const [followedLink, setFollowedLink] = useState(linkedTab);
  if (linkedTab !== followedLink) {
    setFollowedLink(linkedTab);
    if (linkedTab === 'agents' || linkedTab === 'terminals' || linkedTab === 'board') setTab(linkedTab);
  }
  const behind = status.outdated;
  const away = live.status === 'closed' || !!status.problem;
  const unreachable =
    behind === 'host'
      ? { title: 'This host needs a newer Sikemux' }
      : behind === 'phone'
        ? { title: 'Update this app', body: 'This host runs a newer Sikemux.' }
        : { title: "Can't reach this host", body: 'It may be asleep, offline, or have remote access turned off.' };
  const snapshot = live.snapshot;
  const [picking, setPicking] = useState(false);
  const [options, setOptions] = useState(false);
  const starts = device?.access === 'full' && status.online && tab === 'agents';
  const bottom = useBottomGap();
  const scope = snapshot?.workspace.projects.find((project) => project.id === device?.project);
  const asking = snapshot?.inbox.items.filter((item) => item.tier === 'needs' && (!scope || itemIn(snapshot, item, scope))).length ?? 0;
  const [filter, setFilter] = useState('all');
  const providers = [
    ...new Set([
      ...(snapshot?.chats.filter((chat) => !scope || agentIn(snapshot, scope, chat.agentId, chat.cwd)).map((chat) => chat.provider) ?? []),
      ...(snapshot?.recent.filter((chat) => !scope || chat.project === scope.id).map((chat) => chat.provider) ?? []),
    ]),
  ];
  const provider = providers.includes(filter) ? filter : 'all';
  const filtering = tab === 'agents' && providers.length > 1;
  const scopeTo = (project: string | null) => {
    setPicking(false);
    updateDevice(core, { project: project ?? undefined })
      .then(reloadDevices)
      .catch(() => {});
  };
  const [scrolled] = useState(() => new Animated.Value(0));
  const [headHeight, setHeadHeight] = useState(0);
  // The large name hands over to the bar's own title as it scrolls under it, and the tabs then stay put.
  const titleShown = scrolled.interpolate({ inputRange: [headHeight - 48, headHeight - 12], outputRange: [0, 1], extrapolate: 'clamp' });
  const [barHeight, setBarHeight] = useState(34);
  // The tabs ride under the header, then stop at the top bar while the list keeps going under them.
  const unpinned = scrolled.interpolate({
    inputRange: [headHeight - barHeight - FADE, headHeight],
    outputRange: [1, 0],
    extrapolate: 'clamp',
  });
  const barTop = scrolled.interpolate({
    inputRange: [-1000, 0, headHeight],
    outputRange: [headHeight + 1000, headHeight, 0],
    extrapolateRight: 'clamp',
  });
  const hostName = device ? deviceName(device) : 'Host';
  const switchTo = (next: Tab) => {
    if (next !== tab) haptics.select();
    setTab(next);
  };

  return (
    <Screen>
      <Nav
        back="Devices"
        title={
          snapshot ? (
            <Animated.Text style={[styles.navName, { opacity: titleShown }]} numberOfLines={1}>
              {hostName}
            </Animated.Text>
          ) : undefined
        }
        end={device ? <IconButton name="IconMore" label="Options" onPress={() => setOptions(true)} /> : null}
      />
      {device ? <ForgetSheet device={device} visible={options} onClose={() => setOptions(false)} /> : null}
      {snapshot && !status.unpaired ? null : <HostHead name={hostName} line={status.line} online={status.online} />}
      {status.unpaired ? (
        <Unpaired device={device} />
      ) : away && !snapshot ? (
        <Prompt icon={behind ? 'IconDownload' : deviceKind(device?.model)} title={unreachable.title} detail={unreachable.body} fill>
          {behind ? null : (
            <>
              <View style={styles.trying}>
                <Working />
                <Text style={type.meta}>Trying again</Text>
              </View>
              <Button
                title="Try now"
                onPress={() => {
                  haptics.tap();
                  retry(core);
                }}
                style={styles.retry}
              />
            </>
          )}
        </Prompt>
      ) : !snapshot ? (
        <View style={styles.away}>
          <View style={styles.trying}>
            <Working />
          </View>
        </View>
      ) : (
        <>
          <View style={styles.list}>
            <MaskedView
              style={styles.list}
              maskElement={<UnderBar height={barHeight} open={unpinned} bottom={bottom + NEW_CHAT_HEIGHT + 24} />}>
              <Animated.ScrollView
                {...scrollPause}
                showsVerticalScrollIndicator={false}
                onScroll={Animated.event([{ nativeEvent: { contentOffset: { y: scrolled } } }], { useNativeDriver: true })}
                scrollEventThrottle={16}
                style={status.online ? undefined : styles.stale}
                accessibilityHint={status.online ? undefined : 'Out of date until the host answers again'}
                contentContainerStyle={{ paddingBottom: bottom + (starts ? NEW_CHAT_HEIGHT + 24 : 24) }}>
                <View onLayout={(event) => setHeadHeight(event.nativeEvent.layout.height)}>
                  <HostHead name={hostName} line={status.line} online={status.online} />
                </View>
                <View style={{ height: barHeight }} />
                <View style={styles.body}>
                  {snapshot ? (
                    tab === 'agents' ? (
                      <Agents
                        core={core}
                        snapshot={snapshot}
                        scope={scope}
                        provider={provider}
                        full={device?.access === 'full'}
                        hostName={hostName}
                        onTab={switchTo}
                      />
                    ) : tab === 'terminals' ? (
                      <Terminals snapshot={snapshot} scope={scope} />
                    ) : (
                      <Board core={core} connection={connectionOf(live)} snapshot={snapshot} scope={scope} onScope={scopeTo} />
                    )
                  ) : null}
                </View>
              </Animated.ScrollView>
            </MaskedView>
            <Animated.View
              onLayout={(event) => setBarHeight(event.nativeEvent.layout.height)}
              style={[styles.barFloat, { transform: [{ translateY: barTop }] }]}>
              <View style={styles.bar}>
                <View style={styles.tabs} accessibilityRole="tablist">
                  <HostTab label="Agents" count={asking} on={tab === 'agents'} onPress={() => switchTo('agents')} />
                  <HostTab label="Terminals" on={tab === 'terminals'} onPress={() => switchTo('terminals')} />
                  <HostTab label="Board" on={tab === 'board'} onPress={() => switchTo('board')} />
                </View>
                {snapshot.workspace.projects.length || filtering ? (
                  <Pressable
                    onPress={() => setPicking(true)}
                    hitSlop={12}
                    style={styles.picker}
                    accessibilityRole="button"
                    accessibilityLabel="Project">
                    <Icon name="IconFolder" size={14} color={colors.live} />
                    <Text style={styles.pickerText} numberOfLines={1}>
                      {scope?.name ?? 'All projects'}
                    </Text>
                    {filtering && provider !== 'all' ? <AgentIcon provider={provider} size={14} /> : null}
                    <View style={{ transform: [{ rotate: '90deg' }] }}>
                      <Icon name="IconChevron" size={12} color={colors.inkFaint} />
                    </View>
                  </Pressable>
                ) : null}
              </View>
            </Animated.View>
          </View>
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
              providers={filtering ? { offered: providers, chosen: provider, onChoose: setFilter } : undefined}
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
    head: { paddingHorizontal: 18, paddingTop: 10, paddingBottom: 16 },
    where: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    whereText: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 13, color: translucent(colors.ink, 0.72) },
    online: {
      width: 7,
      height: 7,
      borderRadius: 4,
      backgroundColor: colors.live,
      shadowColor: colors.live,
      shadowOpacity: 1,
      shadowRadius: 5,
      shadowOffset: { width: 0, height: 0 },
    },
    offline: { width: 7, height: 7, borderRadius: 4, borderWidth: 1.5, borderColor: colors.rest },
    name: {
      marginTop: 4,
      fontFamily: fonts.uiSemibold,
      fontSize: 32,
      letterSpacing: -1.12,
      color: colors.ink,
      textShadowColor: isLight(colors) ? 'transparent' : 'rgba(0, 0, 0, 0.65)',
      textShadowOffset: { width: 0, height: 2 },
      textShadowRadius: 18,
    },
    bar: {
      flexDirection: 'row',
      alignItems: 'center',
      justifyContent: 'space-between',
      gap: 12,
      paddingHorizontal: 18,
      paddingTop: 6,
      paddingBottom: 8,
    },
    list: { flex: 1 },
    barFloat: { position: 'absolute', top: 0, left: 0, right: 0 },
    navName: { fontFamily: fonts.uiSemibold, fontSize: 16, letterSpacing: -0.25, color: colors.ink },
    tabs: { flexDirection: 'row', alignItems: 'center', gap: 20 },
    tab: { flexDirection: 'row', alignItems: 'center', gap: 7 },
    tabText: { fontFamily: fonts.uiSemibold, fontSize: 15, letterSpacing: -0.15, color: colors.tertiary },
    count: {
      minWidth: 16,
      minHeight: 16,
      borderRadius: 8,
      paddingHorizontal: 6,
      backgroundColor: colors.ink,
      alignItems: 'center',
      justifyContent: 'center',
    },
    countText: { fontFamily: fonts.uiSemibold, fontSize: 11, color: colors.ground },
    picker: { flexShrink: 1, flexDirection: 'row', alignItems: 'center', gap: 6 },
    pickerText: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.secondary },
    body: { paddingHorizontal: 16 },
    open: { marginTop: 10 },
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
    tail: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    faces: { flexDirection: 'row' },
    face: { width: 16, height: 16, alignItems: 'center', justifyContent: 'center' },
    termCount: { flexDirection: 'row', alignItems: 'center', gap: 4 },
    age: { fontSize: 12, fontVariant: ['tabular-nums'] },
    more: { minHeight: 40, justifyContent: 'center', paddingLeft: 38, paddingRight: 8, borderRadius: 9 },
    morePressed: { backgroundColor: colors.active },
    moreText: { ...type.meta, fontSize: 13 },
    note: { ...type.meta, fontSize: 12.5, paddingHorizontal: 8, paddingTop: 8 },
    noteFailed: { color: colors.danger },
    liveDot: { width: 7, height: 7, borderRadius: 4, backgroundColor: colors.live },
    away: { flex: 1, justifyContent: 'center', paddingHorizontal: 32, paddingBottom: 120 },
    trying: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 10, marginTop: 16 },
    retry: { marginTop: 24, alignSelf: 'stretch' },
    stale: { opacity: 0.5 },
    problem: { ...type.meta, color: colors.danger, textAlign: 'center', marginTop: 12 },
  });
};
