import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Platform, Pressable, RefreshControl, ScrollView, Share, StyleSheet, Text, View } from 'react-native';
import { router, useIsFocused } from 'expo-router';
import { useUser } from '@clerk/expo';
import * as Clipboard from 'expo-clipboard';
import type { Device } from '@protocol';
import { agentLabel } from '@mac/agents/catalog';

import { AccountSheet } from '@/account/AccountSheet';
import { Avatar } from '@/account/Avatar';
import { providerName } from '@/account/providers';
import { retryHosts, useAccountHosts } from '@/account/session';
import { reloadDevices, retry, useHostStatus, useLive } from '@/devices/hub';
import { channelLabel, deviceKind, deviceName, type PairedDevice } from '@/devices/paired';
import { summary } from '@/devices/status';
import { chatDisplay } from '@/devices/words';
import { haptics } from '@/ui/haptics';
import { AgentIcon, DeviceIcon, Icon } from '@/ui/Icon';
import { Button, IconButton } from '@/ui/controls';
import { Screen, useBottomGap } from '@/ui/screen';
import { NeedsYou, Working } from '@/ui/status';
import { fonts, type Palette, radius, typeFor, useColors, useStyles } from '@/ui/theme';

function DeviceCard({ device }: { device: PairedDevice }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  // Every host on the list streams its view while the list shows; one opened keeps its own connection.
  const showing = useIsFocused();
  const live = useLive(device.core, showing);
  const status = useHostStatus(device.core, showing);
  // A host being reached again keeps its card lit until a try fails, so cards don't flicker on every retry.
  const away = !status.online && !(status.connecting && !status.problem);
  const snapshot = status.online ? live.snapshot : undefined;
  const needs = snapshot?.inbox.items[0]?.tier === 'needs' ? snapshot.inbox.items[0] : undefined;
  const displays = (snapshot?.chats ?? []).map((chat) => ({ chat, display: chatDisplay(chat) }));
  const working = snapshot?.inbox.working ?? 0;
  const background = displays.reduce((sum, { display }) => sum + display.subagents + display.tasks, 0);
  const busy = displays.filter(({ display }) => display.group === 'working' || display.subagents + display.tasks).map(({ chat }) => chat);
  const work = [working ? `${working} working` : null, background ? `${background} in the background` : null].filter(Boolean).join(' · ');
  const channel = channelLabel(device.channel);
  const meta = snapshot ? summary(snapshot) : status.online ? 'Connecting…' : status.line;

  return (
    <Pressable
      onPress={() => router.push(`/device/${device.core}`)}
      accessibilityRole="button"
      style={({ pressed }) => [styles.card, away && styles.away, pressed && { opacity: 0.85 }]}>
      <View style={styles.head}>
        <View style={styles.glyph}>
          <DeviceIcon kind={deviceKind(device.model)} color={away ? colors.tertiary : colors.ink} />
          <View style={[styles.presence, away ? styles.presenceOff : styles.presenceOn]} />
        </View>
        <View style={{ flex: 1 }}>
          <Text style={[styles.name, away && { color: colors.tertiary }]} numberOfLines={1}>
            {deviceName(device)}
          </Text>
          <Text style={styles.meta} numberOfLines={1}>
            {channel ? `${channel} · ${meta}` : meta}
          </Text>
        </View>
        <Icon name="IconChevron" size={14} color={colors.rest} />
      </View>
      {needs ? (
        <View style={styles.ask}>
          <AgentIcon provider={needs.cli ?? ''} size={16} />
          <View style={{ flex: 1 }}>
            <Text style={styles.askTitle} numberOfLines={1}>
              {needs.title ?? needs.card?.title ?? needs.name ?? agentLabel(needs.cli ?? 'agent')}
            </Text>
            <Text style={styles.askDetail} numberOfLines={1}>
              {needs.detail ? `${needs.what} · ${needs.detail}` : needs.what}
            </Text>
          </View>
          <NeedsYou />
        </View>
      ) : null}
      {busy.length ? (
        <View style={styles.work}>
          <View style={styles.faces}>
            {busy.slice(0, 4).map((chat, index) => (
              <View key={chat.agentId} style={[styles.face, index > 0 && styles.faceAfter]}>
                <AgentIcon provider={chat.provider} size={18} />
              </View>
            ))}
          </View>
          <Text style={styles.workText}>{work}</Text>
          {working ? <Working /> : null}
        </View>
      ) : null}
    </Pressable>
  );
}

/** A host signed in to the same account that this phone has not paired with: it asks that host to let this phone in. */
function AccountHostCard({ host }: { host: Device }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const channel = channelLabel(host.channel);
  return (
    <Pressable
      onPress={() => {
        haptics.tap();
        router.push({ pathname: '/join', params: { core: host.key, name: host.name } });
      }}
      accessibilityRole="button"
      accessibilityLabel={`Connect to ${host.name}`}
      style={({ pressed }) => [styles.card, styles.away, pressed && { opacity: 0.85 }]}>
      <View style={styles.head}>
        <View style={styles.glyph}>
          <DeviceIcon kind="laptop" color={colors.tertiary} />
        </View>
        <View style={{ flex: 1 }}>
          <Text style={[styles.name, { color: colors.tertiary }]} numberOfLines={1}>
            {host.name}
          </Text>
          <Text style={styles.meta} numberOfLines={1}>
            {channel ? `${channel} · ` : ''}On your account
          </Text>
        </View>
        <View style={styles.connect}>
          <Text style={styles.connectText}>Connect</Text>
        </View>
      </View>
    </Pressable>
  );
}

const DOWNLOAD = 'https://sikemux.com/phone';

/** Where the next host lands: it turns into that host's row once it signs in to the account. */
function HostSlot({ onDismiss }: { onDismiss?: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.slot}>
      <View style={styles.slotTile}>
        <DeviceIcon kind="laptop" size={22} color={colors.tertiary} />
      </View>
      <View style={{ flex: 1 }}>
        <Text style={styles.slotTitle}>Your computer goes here</Text>
        <View style={styles.watching}>
          <Working />
          <Text style={styles.watchingText}>Watching the account</Text>
        </View>
      </View>
      {onDismiss ? <IconButton name="IconClose" label="Stop adding a host" onPress={onDismiss} /> : null}
    </View>
  );
}

/** Where the list stands while the account's hosts are still being read, or could not be. */
function AccountNote({ problem }: { problem?: string }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.note} accessibilityLiveRegion="polite">
      {problem ? null : <Working />}
      <Text style={styles.noteText}>{problem ? "Couldn't list the hosts on your account." : 'Looking for hosts on your account'}</Text>
      {problem ? <Button title="Try again" onPress={retryHosts} /> : null}
    </View>
  );
}

function Step({ number, children }: { number: number; children: ReactNode }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.step}>
      <View style={styles.stepNumber}>
        <Text style={styles.stepNumberText} maxFontSizeMultiplier={1.3}>
          {number}
        </Text>
      </View>
      <Text style={styles.stepText}>{children}</Text>
    </View>
  );
}

function Chip({ title, primary, onPress }: { title: string; primary?: boolean; onPress: () => void }) {
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={onPress}
      hitSlop={4}
      accessibilityRole="button"
      style={({ pressed }) => [styles.chip, primary && styles.chipPrimary, pressed && { opacity: 0.85 }]}>
      <Text style={[styles.chipText, primary && styles.chipTextPrimary]}>{title}</Text>
    </Pressable>
  );
}

/** How to bring a host in: install Sikemux there and sign in to this account; it then asks to let this phone in. */
function BringItIn() {
  const styles = useStyles(makeStyles);
  const { user } = useUser();
  const [copied, setCopied] = useState(false);
  const account = [providerName(user?.externalAccounts[0]?.provider), user?.primaryEmailAddress?.emailAddress].filter(Boolean).join(' · ');

  useEffect(() => {
    if (!copied) return;
    const reset = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(reset);
  }, [copied]);

  const send = () => {
    Share.share(Platform.OS === 'ios' ? { url: DOWNLOAD } : { message: DOWNLOAD }).catch(() => {});
  };
  const copy = () => {
    Clipboard.setStringAsync(DOWNLOAD)
      .then(() => setCopied(true))
      .catch(() => {});
  };

  return (
    <View style={styles.how}>
      <Text style={styles.howTitle}>Bring it in</Text>
      <View style={styles.steps}>
        <Step number={1}>
          Get Sikemux from <Text style={styles.strong}>sikemux.com</Text>
        </Step>
        <Step number={2}>
          Sign in with <Text style={styles.strong}>{account || 'this account'}</Text>
        </Step>
        <Step number={3}>
          Click <Text style={styles.strong}>Allow</Text> when it asks
        </Step>
      </View>
      <View style={styles.chips}>
        <Chip primary title="Send me the link" onPress={send} />
        <Chip title={copied ? 'Copied' : 'Copy link'} onPress={copy} />
        <Chip title="Have a code?" onPress={() => router.push('/host-sign-in')} />
      </View>
    </View>
  );
}

export function DevicesList({ devices }: { devices: PairedDevice[] }) {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  const [account, setAccount] = useState(false);
  const { hosts, loaded, problem } = useAccountHosts();
  const unpaired = hosts.filter((host) => !devices.some((device) => device.core === host.key));
  const count = devices.length + unpaired.length;
  // The slot stays while the list is as long as when + was pressed, so the host that fills it takes its place.
  const [addingAt, setAddingAt] = useState<number>();
  const adding = addingAt === count;
  // With nothing paired, the steps wait until the account has said it has no hosts either.
  const empty = count === 0 && loaded;
  const list = useRef<ScrollView>(null);
  const reveal = useRef(false);
  const [refreshing, setRefreshing] = useState(false);

  const add = () => {
    haptics.tap();
    reveal.current = true;
    setAddingAt(count);
  };

  const refresh = () => {
    setRefreshing(true);
    devices.forEach((device) => retry(device.core));
    retryHosts();
    reloadDevices()
      .catch(() => {})
      .finally(() => setRefreshing(false));
  };

  return (
    <Screen>
      <View style={styles.nav}>
        <Text style={styles.title} accessibilityRole="header">
          Devices
        </Text>
        <IconButton name="IconPlus" label="Add a host" onPress={add} />
        <Pressable onPress={() => setAccount(true)} accessibilityRole="button" accessibilityLabel="Account" style={styles.account}>
          <Avatar size={28} />
        </Pressable>
      </View>
      <ScrollView
        ref={list}
        contentContainerStyle={[styles.list, { paddingBottom: bottom + 12 }]}
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={refresh} />}
        onContentSizeChange={() => {
          if (!reveal.current) return;
          reveal.current = false;
          list.current?.scrollToEnd({ animated: true });
        }}>
        {devices.map((device) => (
          <DeviceCard key={device.core} device={device} />
        ))}
        {unpaired.map((host) => (
          <AccountHostCard key={host.key} host={host} />
        ))}
        {count === 0 && !loaded ? <AccountNote problem={problem} /> : null}
        {empty || adding ? (
          <>
            <HostSlot onDismiss={adding && count > 0 ? () => setAddingAt(undefined) : undefined} />
            <BringItIn />
          </>
        ) : null}
        {count > 0 && problem ? <AccountNote problem={problem} /> : null}
      </ScrollView>
      <AccountSheet visible={account} onClose={() => setAccount(false)} />
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    nav: { height: 56, flexDirection: 'row', alignItems: 'center', gap: 8, paddingLeft: 16, paddingRight: 8, marginBottom: 6 },
    account: { width: 44, height: 44, alignItems: 'center', justifyContent: 'center' },
    title: { ...type.title, flex: 1, fontSize: 26 },
    list: { paddingHorizontal: 16, gap: 10 },
    card: { borderRadius: 16, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.raised, overflow: 'hidden' },
    away: { backgroundColor: 'transparent' },
    head: { flexDirection: 'row', alignItems: 'center', gap: 14, paddingVertical: 14, paddingLeft: 16, paddingRight: 14 },
    glyph: { width: 34, height: 34, alignItems: 'center', justifyContent: 'center' },
    presence: { position: 'absolute', right: -1, bottom: 3, width: 10, height: 10, borderRadius: 5, borderWidth: 2.5 },
    presenceOn: { backgroundColor: colors.live, borderColor: colors.raised },
    presenceOff: { backgroundColor: colors.ground, borderColor: colors.rest },
    name: { ...type.heading },
    meta: { ...type.meta, marginTop: 2 },
    connect: {
      minHeight: 32,
      paddingHorizontal: 13,
      justifyContent: 'center',
      borderRadius: radius.control,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      backgroundColor: colors.raised,
    },
    connectText: { fontFamily: fonts.uiSemibold, fontSize: 14, color: colors.ink },
    ask: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      marginHorizontal: 8,
      paddingVertical: 10,
      paddingHorizontal: 12,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      backgroundColor: colors.overlay,
    },
    askTitle: { fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink },
    askDetail: { ...type.meta, fontSize: 12.5, marginTop: 1 },
    work: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingHorizontal: 16, paddingTop: 12, paddingBottom: 14 },
    faces: { flexDirection: 'row' },
    face: { width: 18, height: 18, alignItems: 'center', justifyContent: 'center' },
    faceAfter: { marginLeft: -5 },
    workText: { flex: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.secondary },
    slot: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      padding: 14,
      borderRadius: radius.row,
      borderWidth: 1.5,
      borderStyle: 'dashed',
      borderColor: colors.borderStrong,
    },
    slotTile: {
      width: 40,
      height: 40,
      borderRadius: radius.control,
      alignItems: 'center',
      justifyContent: 'center',
      backgroundColor: colors.active,
    },
    note: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 10, paddingVertical: 24, paddingHorizontal: 8 },
    noteText: { ...type.meta, flexShrink: 1, textAlign: 'center' },
    slotTitle: { ...type.row, color: colors.secondary },
    watching: { flexDirection: 'row', alignItems: 'center', gap: 7, marginTop: 3 },
    watchingText: { ...type.meta, fontSize: 12.5 },
    how: {
      marginTop: 8,
      padding: 16,
      borderRadius: radius.card,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    howTitle: { fontFamily: fonts.uiSemibold, fontSize: 15, color: colors.ink },
    steps: { marginTop: 10, gap: 9 },
    step: { flexDirection: 'row', alignItems: 'flex-start', gap: 10 },
    stepNumber: {
      minWidth: 20,
      minHeight: 20,
      borderRadius: 10,
      alignItems: 'center',
      justifyContent: 'center',
      backgroundColor: colors.active,
    },
    stepNumberText: { fontFamily: fonts.uiSemibold, fontSize: 11.5, color: colors.ink },
    stepText: { flex: 1, fontFamily: fonts.ui, fontSize: 14, lineHeight: 20, color: colors.secondary },
    strong: { fontFamily: fonts.uiSemibold, color: colors.ink },
    chips: { flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginTop: 14 },
    chip: {
      minHeight: 36,
      paddingHorizontal: 14,
      justifyContent: 'center',
      borderRadius: radius.control,
      borderWidth: 1,
      borderColor: colors.borderStrong,
    },
    chipPrimary: { backgroundColor: colors.ink, borderColor: colors.ink },
    chipText: { fontFamily: fonts.uiSemibold, fontSize: 14, color: colors.ink },
    chipTextPrimary: { color: colors.ground },
  });
};
