import { type ComponentProps, useRef, useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';
import { useLocalSearchParams } from 'expo-router';

import { CALLS, NO_CALLS } from '@shared/calls/words';
import { useExternalValue } from '@shared/lib/externalValue';
import { CallCard } from '@/calls/CallCard';
import { draftOf, openCalls } from '@/calls/hostCalls';
import { connectionOf, useDevices, useLive } from '@/devices/hub';
import { useT } from '@/i18n';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { fonts, type Palette, useStyles } from '@/ui/theme';

/** Every call waiting on the person on one page, as the Mac's Calls view lists them, newest first and each in full, opened at the call tapped. */
export default function CallsScreen() {
  const { core, call: shown } = useLocalSearchParams<{ core: string; call?: string }>();
  const styles = useStyles(makeStyles);
  const t = useT();
  const bottom = useBottomGap();
  const live = useLive(core);
  const { devices } = useDevices();
  const watchOnly = devices.find((paired) => paired.core === core)?.access === 'watch';
  const open = openCalls(live.snapshot);
  const scroller = useRef<ScrollView>(null);
  const scrolled = useRef(false);

  return (
    <Screen>
      <Nav title={t(CALLS)} />
      <ScrollView
        ref={scroller}
        contentContainerStyle={[styles.content, { paddingBottom: bottom }]}
        keyboardDismissMode="on-drag"
        keyboardShouldPersistTaps="handled"
        automaticallyAdjustKeyboardInsets>
        {open.length ? null : <Text style={styles.none}>{t(NO_CALLS)}</Text>}
        {open.map((call) => (
          <View
            key={call.id}
            onLayout={(event) => {
              if (call.id !== shown || scrolled.current) return;
              scrolled.current = true;
              scroller.current?.scrollTo({ y: event.nativeEvent.layout.y, animated: false });
            }}>
            <Answering
              core={core}
              calls={live.snapshot?.calls}
              call={call}
              connection={connectionOf(live)}
              canAnswer={!watchOnly && live.status === 'open'}
            />
          </View>
        ))}
      </ScrollView>
    </Screen>
  );
}

/** A call's card, with why an answer did not go under it while no other is on its way. */
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
    none: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary, paddingVertical: 18, paddingHorizontal: 4 },
    refused: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.danger, paddingTop: 6, paddingHorizontal: 4 },
  });
};
