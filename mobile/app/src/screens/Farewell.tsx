import { StyleSheet, Text, View } from 'react-native';

import { sayFarewell, type Farewell as Reason } from '@/account/farewell';
import { Button } from '@/ui/controls';
import { Screen, useBottomGap } from '@/ui/screen';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

const WORDS: Record<Reason, { title: string; body: string }> = {
  removed: {
    title: 'This phone was removed from your account',
    body: 'It was removed from another device or at app.sikemux.com, so it signed out and forgot the hosts it was paired with. Sign in again to use it.',
  },
  deleted: {
    title: 'Your account was deleted',
    body: 'It was deleted on another device or at app.sikemux.com. This phone signed out and forgot the hosts it was paired with. Sign in again within 30 days to restore it.',
  },
  'deleted-here': {
    title: 'Your account is deleted',
    body: 'Every device on it is signed out. Changed your mind? Sign in again within 30 days to restore it. Your code and files never reached us, so they are still on your hosts.',
  },
};

/** Says why the phone signed out, before the welcome screen. */
export function Farewell({ reason }: { reason: Reason }) {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  const words = WORDS[reason];
  return (
    <Screen>
      <View style={styles.block}>
        <Text style={styles.title}>{words.title}</Text>
        <Text style={styles.body}>{words.body}</Text>
      </View>
      <View style={[styles.footer, { paddingBottom: bottom }]}>
        <Button kind="primary" title="Continue" onPress={() => sayFarewell(null)} />
      </View>
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    block: { flex: 1, justifyContent: 'center', paddingHorizontal: 32, paddingBottom: 120 },
    title: { ...type.title, fontSize: 22, textAlign: 'center' },
    body: { ...type.body, textAlign: 'center', marginTop: 10 },
    footer: { paddingHorizontal: 16, paddingTop: 12 },
  });
};
