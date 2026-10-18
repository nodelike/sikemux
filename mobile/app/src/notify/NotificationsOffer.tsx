import { StyleSheet, Text, View } from 'react-native';
import { useAuth } from '@clerk/expo';

import { Button } from '@/ui/controls';
import { PromptGlyph } from '@/ui/prompt';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useStyles } from '@/ui/theme';
import { choose, closeOffer, useOffered } from './setting';
import { notificationsSupported, turnOn } from './switch';

/** Asked once, after the first host lets a signed-in phone in, since notifications reach it through the account. */
export function NotificationsOffer() {
  const styles = useStyles(makeStyles);
  const { isSignedIn, getToken } = useAuth();
  const offered = useOffered();
  const visible = offered && !!isSignedIn && notificationsSupported;

  const notNow = () => {
    closeOffer();
    void choose('off');
  };

  const accept = () => {
    closeOffer();
    turnOn(() => getToken()).catch((error: unknown) => console.warn('sikemux: could not turn on notifications', error));
  };

  return (
    <Sheet visible={visible} onClose={closeOffer}>
      <View style={styles.head}>
        <PromptGlyph icon="IconInbox" small />
        <Text style={styles.title}>Turn on notifications?</Text>
      </View>
      <View style={styles.choices}>
        <Button title="Not now" onPress={notNow} style={styles.choice} />
        <Button kind="primary" title="Turn on" onPress={accept} style={styles.choice} />
      </View>
    </Sheet>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    head: { alignItems: 'center', gap: 12, paddingTop: 12, paddingBottom: 20 },
    title: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    choices: { flexDirection: 'row', gap: 8 },
    choice: { flex: 1 },
  });
