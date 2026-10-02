import { useRef, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import * as Haptics from 'expo-haptics';

import { Button } from '@/ui/parts';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useStyles, useType } from '@/ui/theme';
import { forgetBackdrop } from './backdrop';
import { forget } from './hub';
import { deviceName, type PairedDevice } from './paired';

/** The device screen's options: for now, forgetting the Mac. */
export function ForgetSheet({ device, visible, onClose }: { device: PairedDevice; visible: boolean; onClose: () => void }) {
  const styles = useStyles(makeStyles);
  const type = useType();
  const [forgetting, setForgetting] = useState(false);
  const [problem, setProblem] = useState<string>();
  const busy = useRef(false);
  const name = deviceName(device);

  const leave = async () => {
    if (busy.current) return;
    busy.current = true;
    setForgetting(true);
    setProblem(undefined);
    try {
      await forget(device.core);
    } catch (error) {
      busy.current = false;
      setForgetting(false);
      setProblem(error instanceof Error ? error.message : String(error));
      return;
    }
    try {
      forgetBackdrop(device.backdrop);
    } catch {}
    Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
    onClose();
    router.replace('/');
  };

  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.body}>
        <Text style={styles.title} numberOfLines={1}>
          {name}
        </Text>
        <Text style={type.body}>
          Forgetting removes this Mac from the phone and, if it can be reached, removes this phone from the Mac&apos;s paired devices. To
          use it again, pair with its code.
        </Text>
        {problem ? <Text style={styles.problem}>{problem}</Text> : null}
        <Button
          kind="danger"
          title={forgetting ? 'Forgetting…' : 'Forget this Mac'}
          onPress={leave}
          disabled={forgetting}
          style={styles.button}
        />
      </View>
    </Sheet>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    body: { paddingHorizontal: 4, gap: 12 },
    title: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    problem: { fontFamily: fonts.ui, fontSize: 13.5, lineHeight: 19, color: colors.danger },
    button: { marginTop: 8 },
  });
