import { useEffect, useRef, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import * as Haptics from 'expo-haptics';

import { wasFound } from '@/devices/foundLinks';
import { shortKey } from '@/devices/paired';
import { failure, pair, type Failure } from '@/devices/pairing';
import { Button, CodeTiles, Nav, Screen, useBottomGap, Working } from '@/ui/parts';
import { fonts, type Palette, radius, typeFor, useStyles, useType } from '@/ui/theme';

/** How long the Mac keeps a pairing request open (sikemux_core::pairing::APPROVAL_TIMEOUT). */
const APPROVAL_SECONDS = 120;

const EXPIRED: Failure = {
  title: 'The Mac did not answer',
  detail: 'Pairing waits two minutes for someone at the Mac. Scan the code again to retry.',
};
const BROKEN: Failure = { title: 'That link is incomplete', detail: 'Scan the code on the Mac, or copy its pairing link again.' };

function clock(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

export default function Pair() {
  const styles = useStyles(makeStyles);
  const type = useType();
  const params = useLocalSearchParams<{ core?: string; code?: string }>();
  const core = params.core ?? '';
  const code = params.code ?? '';
  const [confirmed, setConfirmed] = useState(() => wasFound({ core, code }));
  const [failed, setFailed] = useState<Failure>();
  const [left, setLeft] = useState(APPROVAL_SECONDS);
  const pairing = useRef<AbortController>(undefined);
  const problem = !core || !code ? BROKEN : (failed ?? (left === 0 ? EXPIRED : undefined));
  const waiting = confirmed && !problem;

  useEffect(() => {
    if (!confirmed || !core || !code) return;
    const controller = new AbortController();
    pairing.current = controller;
    pair({ core, code }, controller.signal)
      .then(() => {
        if (controller.signal.aborted) return;
        Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
        router.replace(`/device/${core}`);
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) setFailed(failure(error));
      });
    return () => controller.abort();
  }, [confirmed, core, code]);

  useEffect(() => {
    if (!waiting) return;
    const tick = setInterval(() => setLeft((seconds) => Math.max(0, seconds - 1)), 1000);
    return () => clearInterval(tick);
  }, [waiting]);

  useEffect(() => {
    if (left === 0) pairing.current?.abort();
  }, [left]);

  const leave = () => {
    pairing.current?.abort();
    if (router.canGoBack()) router.back();
    else router.replace('/');
  };

  const bottom = useBottomGap();
  return (
    <Screen>
      <Nav back={problem ? 'Back' : 'Cancel'} onBack={leave} />
      <View style={styles.block}>
        <Text style={styles.title}>{problem ? problem.title : confirmed ? 'Approve on your Mac' : 'Pair with this Mac?'}</Text>
        <Text style={styles.detail}>
          {problem
            ? problem.detail
            : confirmed
              ? 'Check the Mac shows this code, then choose what this phone may do.'
              : 'Only go on if your Mac shows this code right now.'}
        </Text>
        <View style={styles.tiles}>
          <CodeTiles code={code} state={problem ? 'failed' : 'locked'} />
        </View>
        {confirmed || problem ? null : <Text style={[type.mono, styles.key]}>Mac {shortKey(core)}</Text>}
      </View>
      <View style={[styles.footer, { paddingBottom: bottom }]}>
        {problem ? (
          <Button kind="primary" title="Scan again" onPress={() => router.replace('/scan')} />
        ) : confirmed ? (
          <View style={styles.waiting}>
            <Working />
            <Text style={styles.waitingText}>Waiting for your Mac</Text>
            <Text style={type.mono}>{clock(left)}</Text>
          </View>
        ) : (
          <Button kind="primary" title="Pair" onPress={() => setConfirmed(true)} />
        )}
      </View>
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    block: { flex: 1, paddingTop: 40, paddingHorizontal: 28, alignItems: 'center' },
    title: { ...type.title, fontSize: 22, textAlign: 'center' },
    detail: { ...type.body, textAlign: 'center', marginTop: 8, minHeight: 44 },
    tiles: { marginTop: 32 },
    key: { marginTop: 16 },
    footer: { paddingHorizontal: 16, paddingTop: 12 },
    waiting: {
      minHeight: 52,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      paddingHorizontal: 16,
      borderRadius: radius.row,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    waitingText: { flex: 1, fontFamily: fonts.ui, fontSize: 15, color: colors.secondary },
  });
};
