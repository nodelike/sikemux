import { useEffect, useState } from 'react';
import { AppState, RefreshControl, ScrollView, StyleSheet, Switch, Text, View } from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import type { ConnectionLike } from '@sikemux/native';

import { ACCESS_OPTIONS, platformName } from '@shared/remote/access';
import { formatAgo } from '@shared/lib/format';
import { type DeviceAccess, type LinkedHost, type DeviceInfo, type RemotePairing, readPairing } from '@/core/protocol';
import { useDeviceId } from '@/device/identity';
import { problem as problemOf } from '@/core/problem';
import { connectionOf, useDevices, useLive } from '@/devices/hub';
import { deviceName } from '@/devices/paired';
import { counted } from '@/devices/words';
import { useCardArrivals } from '@/notify/responses';
import { haptics } from '@/ui/haptics';
import { DeviceIcon, Icon } from '@/ui/Icon';
import { Button, Chip, Track } from '@/ui/controls';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { useMinute } from '@/ui/motion';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';

const accessLabel = (access: DeviceAccess) => ACCESS_OPTIONS.find((option) => option.value === access)?.label ?? access;

/** The host's pairing requests and paired devices, read with its `pairing` call and changed with the calls that answer it. */
function usePairing(connection: ConnectionLike | undefined) {
  const [pairing, setPairing] = useState<RemotePairing>();
  const [problem, setProblem] = useState<string>();
  const [busy, setBusy] = useState<string>();
  const [reads, setReads] = useState(0);

  useEffect(() => {
    if (!connection) return;
    let current = true;
    connection.call('pairing', 'null').then(
      (text) => {
        if (!current) return;
        setPairing(readPairing(text));
        setProblem(undefined);
      },
      (error: unknown) => {
        if (current) setProblem(problemOf(error));
      },
    );
    return () => {
      current = false;
    };
  }, [connection, reads]);

  useEffect(() => {
    const listener = AppState.addEventListener('change', (state) => {
      if (state === 'active') setReads((count) => count + 1);
    });
    return () => listener.remove();
  }, []);

  const change = async (id: string, method: 'answerPairing' | 'changeDevice', params: object) => {
    if (!connection || busy) return;
    setBusy(id);
    try {
      setPairing(readPairing(await connection.call(method, JSON.stringify({ id, ...params }))));
      setProblem(undefined);
      haptics.success();
    } catch (error) {
      haptics.failure();
      setProblem(problemOf(error));
      setReads((count) => count + 1);
    } finally {
      setBusy(undefined);
    }
  };

  return {
    pairing,
    problem,
    busy,
    reread: () => setReads((count) => count + 1),
    answer: (id: string, access: DeviceAccess | null) => change(id, 'answerPairing', { allow: access !== null, access: access ?? 'watch' }),
    setAccess: (id: string, access: DeviceAccess | null) => change(id, 'changeDevice', { access }),
  };
}

/** One paired device: what it may do, and taking it off the host. */
function DeviceSheet({
  device,
  onClose,
  onAccess,
}: {
  device?: DeviceInfo;
  onClose: () => void;
  onAccess: (id: string, access: DeviceAccess | null) => void;
}) {
  const styles = useStyles(makeStyles);
  const type = useType();
  return (
    <Sheet visible={!!device} onClose={onClose}>
      {device ? (
        <View style={styles.sheet}>
          <Text style={styles.sheetTitle} numberOfLines={1}>
            {device.name || 'Unnamed phone'}
          </Text>
          <Track
            options={ACCESS_OPTIONS.map((option) => ({ value: option.value, label: option.label }))}
            value={device.access}
            onChange={(access) => onAccess(device.id, access)}
          />
          <Text style={type.meta}>{ACCESS_OPTIONS.find((option) => option.value === device.access)?.detail}</Text>
          <Button
            kind="danger"
            title="Remove from this host"
            onPress={() => {
              onAccess(device.id, null);
              onClose();
            }}
            style={styles.remove}
          />
        </View>
      ) : null}
    </Sheet>
  );
}

const hostKind = (platform: string) => (platform === 'linux' ? 'desktop' : 'laptop');

const agentCount = (count: number) => `${count} ${counted('agent', count)}`;

function linkDetail(link: LinkedHost, now: number): string {
  const reach = link.connected ? 'connected' : link.lastSeen ? `seen ${formatAgo(link.lastSeen, now)}` : 'never connected';
  return [platformName(link.platform), reach, link.agents ? agentCount(link.agents) : null].filter(Boolean).join(' · ');
}

/** The host's lock (ADR 0018): whether each new device and host on the account waits for the person here. */
function ApprovalRow({ connection }: { connection: ConnectionLike | undefined }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const [on, setOn] = useState<boolean>();
  const [problem, setProblem] = useState<string>();
  useEffect(() => {
    if (!connection) return;
    let current = true;
    connection.call('requireApproval', 'null').then(
      (text) => current && setOn((JSON.parse(text) as { requireApproval: boolean }).requireApproval),
      (error: unknown) => current && setProblem(problemOf(error)),
    );
    return () => {
      current = false;
    };
  }, [connection]);
  const flip = async (next: boolean) => {
    if (!connection) return;
    setOn(next);
    try {
      await connection.call('setRequireApproval', JSON.stringify({ requireApproval: next }));
      setProblem(undefined);
    } catch (error) {
      setOn(!next);
      setProblem(problemOf(error));
    }
  };
  return (
    <View style={styles.lock}>
      <Icon name="IconLock" size={18} color={colors.secondary} />
      <View style={{ flex: 1 }}>
        <Text style={styles.lockTitle}>Require approval for new devices</Text>
        <Text style={type.meta}>{problem ?? 'Ask here before a device or host on your account connects'}</Text>
      </View>
      <Switch
        value={on ?? false}
        disabled={on === undefined}
        onValueChange={(next) => void flip(next)}
        trackColor={{ false: colors.active, true: colors.live }}
        thumbColor={colors.ink}
        accessibilityLabel="Require approval for new devices"
      />
    </View>
  );
}

/** Who may connect to a host: the devices asking now, answered here, and those already paired, changed or removed here. */
export default function HostPairing() {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const bottom = useBottomGap();
  const minute = useMinute();
  const { core } = useLocalSearchParams<{ core: string }>();
  const { devices } = useDevices();
  const device = devices.find((known) => known.core === core);
  const live = useLive(core);
  const connection = connectionOf(live);
  const { pairing, problem, busy, reread, answer, setAccess } = usePairing(connection);
  const [editing, setEditing] = useState<string>();
  const phone = useDeviceId();
  const hostName = device ? deviceName(device) : 'Host';
  useCardArrivals(core, 'pairing', reread);

  const now = minute * 60_000;
  const pending = pairing?.pending.filter((request) => request.expiresAt > now) ?? [];
  const asking = pairing?.pendingLinks?.filter((request) => request.expiresAt > now) ?? [];
  const links = pairing?.links ?? [];
  return (
    <Screen>
      <Nav back="Back" title="Paired devices" />
      <ScrollView
        showsVerticalScrollIndicator={false}
        contentContainerStyle={[styles.body, { paddingBottom: bottom + 24 }]}
        refreshControl={<RefreshControl refreshing={false} onRefresh={reread} />}>
        <ApprovalRow connection={connection} />
        {pending.length ? (
          <>
            <SectionLabel>Asking to connect</SectionLabel>
            <Rows>
              {pending.map((request) => (
                <View key={request.id}>
                  <Row
                    mark={<Icon name="IconPhone" size={18} color={colors.ink} />}
                    title={request.name || 'A phone'}
                    detail={`${platformName(request.platform)} · from your account`}
                  />
                  <View style={styles.chips}>
                    <Chip title="Refuse" busy={busy === request.id} onPress={() => void answer(request.id, null)} />
                    <Chip title="Watch only" busy={busy === request.id} onPress={() => void answer(request.id, 'watch')} />
                    <Chip title="Full control" primary busy={busy === request.id} onPress={() => void answer(request.id, 'full')} />
                  </View>
                </View>
              ))}
            </Rows>
          </>
        ) : null}
        {pairing ? (
          <>
            <SectionLabel>Paired</SectionLabel>
            <Rows>
              {pairing.devices.map((paired) => {
                const self = paired.id === phone;
                const seen = paired.lastSeen ? `seen ${formatAgo(paired.lastSeen, now)}` : 'never connected';
                return (
                  <Row
                    key={paired.id}
                    mark={<Icon name="IconPhone" size={18} color={colors.secondary} />}
                    title={paired.name || 'Unnamed phone'}
                    detail={`${accessLabel(paired.access)} · ${self ? 'this phone' : seen}`}
                    end={self ? null : <Icon name="IconChevron" size={14} color={colors.rest} />}
                    onPress={self ? undefined : () => setEditing(paired.id)}
                  />
                );
              })}
            </Rows>
          </>
        ) : null}
        {asking.length ? (
          <>
            <SectionLabel>Asking to link</SectionLabel>
            <Rows>
              {asking.map((request) => (
                <View key={request.id}>
                  <Row
                    mark={<DeviceIcon kind={hostKind(request.platform)} size={18} />}
                    title={request.name || 'A host'}
                    detail={`${platformName(request.platform)} · from your account`}
                  />
                  <View style={styles.chips}>
                    <Chip title="Decline" busy={busy === request.id} onPress={() => void answer(request.id, null)} />
                    <Chip title="Allow" primary busy={busy === request.id} onPress={() => void answer(request.id, 'full')} />
                  </View>
                </View>
              ))}
            </Rows>
          </>
        ) : null}
        {links.length ? (
          <>
            <SectionLabel>Linked hosts</SectionLabel>
            <Rows>
              {links.map((link) => (
                <Row
                  key={link.id}
                  mark={<DeviceIcon kind={hostKind(link.platform)} size={18} color={colors.secondary} />}
                  title={link.name}
                  detail={linkDetail(link, now)}
                />
              ))}
            </Rows>
          </>
        ) : null}
        {!pairing && !problem ? <Text style={styles.note}>Asking {hostName}…</Text> : null}
        {problem ? (
          <Text style={[styles.note, styles.problem]} accessibilityLiveRegion="polite">
            {problem}
          </Text>
        ) : null}
      </ScrollView>
      <DeviceSheet
        device={pairing?.devices.find((paired) => paired.id === editing)}
        onClose={() => setEditing(undefined)}
        onAccess={(id, access) => void setAccess(id, access)}
      />
    </Screen>
  );
}

/** Lines under a row start where its text does. */
const TEXT_INSET = 38;

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    body: { paddingHorizontal: 16 },
    chips: { flexDirection: 'row', flexWrap: 'wrap', gap: 8, paddingLeft: TEXT_INSET, paddingTop: 2, paddingBottom: 8 },
    note: { ...type.meta, fontSize: 13, paddingHorizontal: 8, paddingTop: 16 },
    problem: { color: colors.danger },
    sheet: { paddingHorizontal: 4, gap: 12 },
    sheetTitle: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    remove: { marginTop: 8 },
    lock: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      minHeight: 52,
      marginTop: 8,
      paddingHorizontal: 10,
      paddingVertical: 6,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    lockTitle: { ...type.row, fontSize: 15, color: colors.ink },
  });
};
