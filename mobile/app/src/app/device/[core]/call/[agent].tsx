import { type ComponentProps, useEffect, useMemo, useState } from 'react';
import { ScrollView, StyleSheet, Text } from 'react-native';
import { useLocalSearchParams } from 'expo-router';

import { CALLS } from '@shared/calls/words';
import { useExternalValue } from '@shared/lib/externalValue';
import { CallCard } from '@/calls/CallCard';
import { draftOf, queueOf, useKept, waitingCall } from '@/calls/hostCalls';
import { connectionOf, useDevices, useLive } from '@/devices/hub';
import { useT } from '@/i18n';
import { goBack } from '@/ui/navigate';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { fonts, type Palette, useStyles } from '@/ui/theme';

/** The calls an agent puts to the person, answered one at a time on a screen of their own. It goes back once none is left open. */
export default function CallScreen() {
  const { core, agent } = useLocalSearchParams<{ core: string; agent: string }>();
  const styles = useStyles(makeStyles);
  const t = useT();
  const bottom = useBottomGap();
  const live = useLive(core);
  const { devices } = useDevices();
  const watchOnly = devices.find((paired) => paired.core === core)?.access === 'watch';
  const calls = live.snapshot?.calls;
  const queue = useMemo(() => queueOf(calls, agent), [calls, agent]);
  const waiting = waitingCall(queue, useKept(core), agent);

  useEffect(() => {
    if (live.snapshot && !waiting) goBack();
  }, [live.snapshot, waiting]);

  return (
    <Screen>
      <Nav title={t(CALLS)} />
      <ScrollView
        contentContainerStyle={[styles.content, { paddingBottom: bottom }]}
        keyboardDismissMode="on-drag"
        keyboardShouldPersistTaps="handled"
        automaticallyAdjustKeyboardInsets>
        {waiting ? (
          <Answering
            key={waiting.id}
            core={core}
            calls={calls}
            call={waiting}
            chat={agent}
            queue={queue}
            connection={connectionOf(live)}
            canAnswer={!watchOnly && live.status === 'open'}
          />
        ) : null}
      </ScrollView>
    </Screen>
  );
}

/** The call's card, with why an answer did not go under it while no other is on its way. */
function Answering(props: Omit<ComponentProps<typeof CallCard>, 'report'>) {
  const styles = useStyles(makeStyles);
  const [refused, setRefused] = useState<string>();
  const sending = useExternalValue(draftOf(props.calls, props.call)).sending;
  return (
    <>
      <CallCard {...props} report={setRefused} />
      {refused && !sending ? <Text style={styles.refused}>{refused}</Text> : null}
    </>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    content: { paddingHorizontal: 10, paddingTop: 8 },
    refused: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.danger, paddingTop: 6, paddingHorizontal: 4 },
  });
};
