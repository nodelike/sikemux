import { useRef, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import { haptics } from '@/ui/haptics';
import { goHome } from '@/ui/navigate';
import { Button } from '@/ui/controls';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useStyles, useType } from '@/ui/theme';
import { forgetBackdrop } from './backdrop';
import { forget } from './hub';
import { deviceName, type PairedDevice } from './paired';

/** Forgets a host and goes back to Devices; a failure stays to be shown. */
export function useForget(device: PairedDevice | undefined, done?: () => void) {
  const [forgetting, setForgetting] = useState(false);
  const [problem, setProblem] = useState<string>();
  const busy = useRef(false);

  const leave = async () => {
    if (!device) return;
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
      forgetBackdrop(device.core, device.backdrop);
    } catch {}
    haptics.success();
    done?.();
    goHome();
  };

  return { forgetting, problem, leave };
}

/** The device screen's options: for now, forgetting the host. */
export function ForgetSheet({ device, visible, onClose }: { device: PairedDevice; visible: boolean; onClose: () => void }) {
  const styles = useStyles(makeStyles);
  const type = useType();
  const { forgetting, problem, leave } = useForget(device, onClose);
  const name = deviceName(device);

  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.body}>
        <Text style={styles.title} numberOfLines={1}>
          {name}
        </Text>
        <Text style={type.body}>
          Forgetting removes this host from the phone and, if it can be reached, removes this phone from the host&apos;s paired devices.
          While it is on your account, it stays in Devices to connect to again.
        </Text>
        {problem ? <Text style={styles.problem}>{problem}</Text> : null}
        <Button
          kind="danger"
          title={forgetting ? 'Forgetting…' : 'Forget this host'}
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
