import { useEffect, useState, type ReactNode } from 'react';
import { Animated, Dimensions, Easing, Modal, Pressable, StyleSheet, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { fonts, type Palette, useStyles } from './theme';

const OPEN_MS = 260;
const CLOSE_MS = 180;

/**
 * A sheet that slides up while the screen behind it dims in place; a tap on
 * the dimmed screen closes it. The modal stays mounted until the sheet is down.
 */
export function Sheet({
  visible,
  onClose,
  tall,
  children,
}: {
  visible: boolean;
  onClose: () => void;
  tall?: boolean;
  children: ReactNode;
}) {
  const styles = useStyles(makeStyles);
  // A modal measures no safe area of its own, so the screen behind it lends its inset.
  const insets = useSafeAreaInsets();
  const [mounted, setMounted] = useState(visible);
  if (visible && !mounted) setMounted(true);
  const [progress] = useState(() => new Animated.Value(0));
  const offscreen = Dimensions.get('window').height;

  useEffect(() => {
    if (visible) {
      Animated.timing(progress, { toValue: 1, duration: OPEN_MS, easing: Easing.out(Easing.cubic), useNativeDriver: true }).start();
      return;
    }
    Animated.timing(progress, { toValue: 0, duration: CLOSE_MS, easing: Easing.in(Easing.cubic), useNativeDriver: true }).start(
      ({ finished }) => {
        if (finished) setMounted(false);
      },
    );
  }, [visible, progress]);

  const rise = progress.interpolate({ inputRange: [0, 1], outputRange: [offscreen, 0] });

  return (
    <Modal visible={mounted} transparent animationType="none" onRequestClose={onClose} statusBarTranslucent>
      <Animated.View style={[StyleSheet.absoluteFill, styles.scrim, { opacity: progress }]}>
        <Pressable style={{ flex: 1 }} onPress={onClose} accessibilityLabel="Close" />
      </Animated.View>
      <View style={styles.dock} pointerEvents="box-none">
        <Animated.View
          style={[styles.sheet, tall && { height: '82%' }, { paddingBottom: insets.bottom + 12, transform: [{ translateY: rise }] }]}>
          <View style={styles.grabber} />
          {children}
        </Animated.View>
      </View>
    </Modal>
  );
}

export function SheetLabel({ children }: { children: ReactNode }) {
  const styles = useStyles(makeStyles);
  return <Text style={styles.label}>{children}</Text>;
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    scrim: { backgroundColor: 'rgba(9, 9, 11, 0.62)' },
    dock: { flex: 1, justifyContent: 'flex-end' },
    sheet: {
      backgroundColor: colors.overlay,
      borderTopLeftRadius: 22,
      borderTopRightRadius: 22,
      borderTopWidth: 1,
      borderColor: colors.border,
      paddingHorizontal: 16,
      paddingTop: 8,
      paddingBottom: 12,
    },
    grabber: { alignSelf: 'center', width: 36, height: 5, borderRadius: 3, backgroundColor: colors.borderStrong, marginBottom: 12 },
    label: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary, paddingTop: 14, paddingBottom: 8, paddingHorizontal: 6 },
  });
};
