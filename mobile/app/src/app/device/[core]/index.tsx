import { useState } from 'react';
import { Animated, Pressable, StyleSheet, Text, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import MaskedView from '@react-native-masked-view/masked-view';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import type { ProjectInfo } from '@shared/wire.generated';
import type { Snapshot } from '@/core/protocol';
import { reloadDevices, retry, useDevices, useHostStatus, useLive } from '@/devices/hub';
import { deviceKind, deviceName, type PairedDevice, updateDevice } from '@/devices/paired';
import { CallsTab } from '@/calls/CallsTab';
import { openCalls } from '@/calls/hostCalls';
import { CALLS } from '@shared/calls/words';
import { ForgetSheet, useForget } from '@/devices/ForgetSheet';
import { ProjectSheet } from '@/devices/ProjectSheet';
import { summary } from '@/devices/status';
import { projectOf } from '@/devices/words';
import { haptics } from '@/ui/haptics';
import { useScrollPause } from '@/ui/motion';
import { AgentFace } from '@/devices/Buddy';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Button, IconButton } from '@/ui/controls';
import { Prompt } from '@/ui/prompt';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { Dot, Working } from '@/devices/AgentMarks';
import { fonts, isLight, type Palette, typeFor, useColors, useStyles, useType, translucent } from '@/ui/theme';
import { Agents, type Tab } from '@/devices/DeviceAgents';
import { Terminals } from '@/devices/DeviceTerminals';

/** What runs in a project, as its row in the project sheet shows it. */
function ProjectTail({ snapshot, project }: { snapshot: Snapshot; project: ProjectInfo }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const chats = snapshot.chats.filter((chat) => projectOf(snapshot, chat.agentId) === project.id);
  const terminals = snapshot.sessions.filter((session) => session.project === project.id && session.running).length;
  const turnOf = (agentId: string) => snapshot.inbox.agents.find((agent) => agent.agentId === agentId)?.turn;
  const waiting = openCalls(snapshot, project).length > 0 || chats.some((chat) => turnOf(chat.agentId)?.state === 'needsYou');
  if (!chats.length && !terminals) return null;
  return (
    <View style={styles.tail}>
      {waiting ? <Dot color={colors.warn} size={7} /> : null}
      {chats.length ? (
        <View style={styles.faces}>
          {chats.slice(0, 3).map((chat, index) => (
            <View key={chat.agentId} style={[styles.face, index > 0 && { marginLeft: -4 }]}>
              <AgentFace agentId={chat.agentId} provider={chat.provider} turn={turnOf(chat.agentId)?.state} size={16} />
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

/** A downward chevron, as a picker's trigger shows it. */
function Chevron({ size }: { size: number }) {
  const colors = useColors();
  return (
    <View style={{ transform: [{ rotate: '90deg' }] }}>
      <Icon name="IconChevron" size={size} color={colors.tertiary} />
    </View>
  );
}

/**
 * The host in the bar, where the screen it came from would be named: its name and whether it is online, and once
 * the big title has scrolled under the bar, the project after it, which still opens the picker.
 */
function HostTitle({
  name,
  online,
  project,
  shown,
  onPick,
}: {
  name: string;
  online: boolean;
  project?: string;
  shown: Animated.AnimatedInterpolation<number>;
  onPick?: () => void;
}) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.hostTitle}>
      <Text style={styles.hostName} numberOfLines={1}>
        {name}
      </Text>
      <View style={online ? styles.online : styles.offline} accessibilityLabel={online ? 'Online' : 'Offline'} />
      {project ? (
        <Animated.View style={[styles.barProjectWrap, { opacity: shown }]}>
          <Pressable
            onPress={onPick}
            disabled={!onPick}
            hitSlop={8}
            style={styles.barProject}
            accessibilityRole="button"
            accessibilityLabel={`Project, ${project}`}>
            <Text style={styles.slash}>/</Text>
            <Text style={styles.hostName} numberOfLines={1}>
              {project}
            </Text>
            {onPick ? <Chevron size={12} /> : null}
          </Pressable>
        </Animated.View>
      ) : null}
    </View>
  );
}

/** The big title: the project in view, which opens the picker, with how the host is doing under it. */
function ProjectHead({ title, line, provider, onPick }: { title: string; line: string; provider?: string; onPick?: () => void }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={[styles.head, styles.projectHead]}>
      <Pressable
        onPress={onPick}
        disabled={!onPick}
        style={({ pressed }) => [styles.projectTitle, pressed && { opacity: 0.6 }]}
        accessibilityRole="button"
        accessibilityLabel={`Project, ${title}`}>
        <Text style={[styles.name, styles.projectName]} numberOfLines={1} maxFontSizeMultiplier={1.4}>
          {title}
        </Text>
        {provider ? <AgentIcon provider={provider} size={18} /> : null}
        {onPick ? <Chevron size={16} /> : null}
      </Pressable>
      <Text style={styles.whereText} numberOfLines={1}>
        {line}
      </Text>
    </View>
  );
}

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

const TABS: { tab: Tab; label: string; icon: 'IconAgent' | 'IconTerminal' | 'IconMessage'; tint: keyof Palette }[] = [
  { tab: 'agents', label: 'Agents', icon: 'IconAgent', tint: 'accent' },
  { tab: 'terminals', label: 'Terminals', icon: 'IconTerminal', tint: 'warn' },
  { tab: 'calls', label: CALLS, icon: 'IconMessage', tint: 'warn' },
];

/** Agents, Terminals and Calls as one segmented control, each with its icon in its own colour. */
function HostTabs({ tab, calling, onSwitch }: { tab: Tab; calling: number; onSwitch: (tab: Tab) => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.tabs} accessibilityRole="tablist">
      {TABS.map((item) => {
        const on = item.tab === tab;
        return (
          <Pressable
            key={item.tab}
            onPress={() => onSwitch(item.tab)}
            style={[styles.tab, on && styles.tabOn]}
            accessibilityRole="tab"
            accessibilityLabel={item.label}
            accessibilityState={{ selected: on }}>
            <Icon name={item.icon} size={15} color={colors[item.tint] as string} />
            <Text style={[styles.tabText, on && { color: colors.ink }]} numberOfLines={1}>
              {item.label}
            </Text>
            {item.tab === 'calls' && calling ? (
              <View style={styles.count}>
                <Text style={styles.countText} maxFontSizeMultiplier={1.3}>
                  {calling}
                </Text>
              </View>
            ) : null}
          </Pressable>
        );
      })}
    </View>
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

export { Crashed as ErrorBoundary } from '@/ui/Crashed';

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
  const [tab, setTab] = useState<Tab>(linkedTab === 'terminals' || linkedTab === 'calls' ? linkedTab : 'agents');
  const [followedLink, setFollowedLink] = useState(linkedTab);
  if (linkedTab !== followedLink) {
    setFollowedLink(linkedTab);
    if (linkedTab === 'agents' || linkedTab === 'terminals' || linkedTab === 'calls') setTab(linkedTab);
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
  const starts = device?.access === 'full' && status.online;
  const bottom = useBottomGap();
  const scope = snapshot?.workspace.projects.find((project) => project.id === device?.project);
  const calling = openCalls(snapshot, scope).length;
  const [filter, setFilter] = useState('all');
  const providers = [
    ...new Set([
      ...(snapshot?.chats.filter((chat) => !scope || projectOf(snapshot, chat.agentId) === scope.id).map((chat) => chat.provider) ?? []),
      ...(snapshot?.recent.filter((chat) => !scope || chat.project === scope.id).map((chat) => chat.provider) ?? []),
    ]),
  ];
  const provider = providers.includes(filter) ? filter : 'all';
  const filtering = tab === 'agents' && providers.length > 1;
  const scopeTo = (project: string | null) => {
    setPicking(false);
    updateDevice(core, { project: project ?? undefined })
      .then(reloadDevices)
      .catch((error: unknown) => console.warn('sikemux: could not keep the project this host shows', error));
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
  const projectLabel = scope?.name ?? 'All projects';
  // The title opens the project picker when there is something to pick.
  const pickable = !!snapshot && !status.unpaired && (snapshot.workspace.projects.length > 0 || providers.length > 1);
  const switchTo = (next: Tab) => {
    if (next !== tab) haptics.select();
    setTab(next);
  };

  return (
    <Screen>
      <Nav
        title={
          <HostTitle
            name={hostName}
            online={status.online}
            project={snapshot ? projectLabel : undefined}
            shown={titleShown}
            onPick={pickable ? () => setPicking(true) : undefined}
          />
        }
        end={
          device ? (
            <View style={styles.navEnd}>
              {snapshot && starts ? (
                <Pressable
                  onPress={() => {
                    haptics.tap();
                    router.push(scope ? `/device/${core}/new?project=${encodeURIComponent(scope.id)}` : `/device/${core}/new`);
                  }}
                  hitSlop={6}
                  style={({ pressed }) => [styles.navButton, pressed && { opacity: 0.6 }]}
                  accessibilityRole="button"
                  accessibilityLabel="New chat">
                  <Icon name="IconPlus" size={20} color={colors.ink} />
                </Pressable>
              ) : null}
              <IconButton name="IconMore" label="Options" onPress={() => setOptions(true)} />
            </View>
          ) : null
        }
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
            <MaskedView style={styles.list} maskElement={<UnderBar height={barHeight} open={unpinned} bottom={bottom + 24} />}>
              <Animated.ScrollView
                {...scrollPause}
                showsVerticalScrollIndicator={false}
                onScroll={Animated.event([{ nativeEvent: { contentOffset: { y: scrolled } } }], { useNativeDriver: true })}
                scrollEventThrottle={16}
                style={status.online ? undefined : styles.stale}
                accessibilityHint={status.online ? undefined : 'Out of date until the host answers again'}
                contentContainerStyle={{ paddingBottom: bottom + 24 }}>
                <View onLayout={(event) => setHeadHeight(event.nativeEvent.layout.height)}>
                  <ProjectHead
                    title={projectLabel}
                    line={status.facts}
                    provider={filtering && provider !== 'all' ? provider : undefined}
                    onPick={pickable ? () => setPicking(true) : undefined}
                  />
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
                      <CallsTab core={core} snapshot={snapshot} scope={scope} />
                    )
                  ) : null}
                </View>
              </Animated.ScrollView>
            </MaskedView>
            <Animated.View
              onLayout={(event) => setBarHeight(event.nativeEvent.layout.height)}
              style={[styles.barFloat, { transform: [{ translateY: barTop }] }]}>
              <View style={styles.bar}>
                <HostTabs tab={tab} calling={calling} onSwitch={switchTo} />
              </View>
            </Animated.View>
          </View>
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
    bar: { paddingHorizontal: 16, paddingTop: 4, paddingBottom: 10 },
    list: { flex: 1 },
    barFloat: { position: 'absolute', top: 0, left: 0, right: 0 },
    hostTitle: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', gap: 6 },
    hostName: { flexShrink: 1, fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.3, color: colors.ink },
    barProjectWrap: { flexShrink: 1, minWidth: 0 },
    barProject: { flexDirection: 'row', alignItems: 'center', gap: 5, minHeight: 44 },
    slash: { fontFamily: fonts.ui, fontSize: 17, color: colors.tertiary },
    projectHead: { paddingTop: 4 },
    projectTitle: { flexDirection: 'row', alignItems: 'center', gap: 8, alignSelf: 'flex-start', marginBottom: 4 },
    projectName: { marginTop: 0, flexShrink: 1 },
    tabs: {
      flexDirection: 'row',
      gap: 2,
      padding: 3,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: translucent(colors.raised, 0.86),
    },
    tab: { flex: 1, minHeight: 34, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 6, borderRadius: 9 },
    tabOn: { backgroundColor: colors.active },
    tabText: { fontFamily: fonts.uiSemibold, fontSize: 14, letterSpacing: -0.15, color: colors.secondary },
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
    navEnd: { flexDirection: 'row', alignItems: 'center' },
    navButton: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
    body: { paddingHorizontal: 16 },
    tail: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    faces: { flexDirection: 'row' },
    face: { width: 16, height: 16, alignItems: 'center', justifyContent: 'center' },
    termCount: { flexDirection: 'row', alignItems: 'center', gap: 4 },
    away: { flex: 1, justifyContent: 'center', paddingHorizontal: 32, paddingBottom: 120 },
    trying: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 10, marginTop: 16 },
    retry: { marginTop: 24, alignSelf: 'stretch' },
    stale: { opacity: 0.5 },
    problem: { ...type.meta, color: colors.danger, textAlign: 'center', marginTop: 12 },
  });
};
