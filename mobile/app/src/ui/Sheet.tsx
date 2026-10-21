import { useEffect, useEffectEvent, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  Animated,
  Dimensions,
  Easing,
  Keyboard,
  Modal,
  PanResponder,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { LockCover } from '@/lock/LockScreen';
import { pauseBackdrop, useStill } from './motion';
import { fonts, type Palette, useStyles } from './theme';

const OPEN_MS = 260;
const CLOSE_MS = 180;
/** A drag past this far, or a flick faster than this, closes the sheet. */
const CLOSE_DRAG = 110;
const CLOSE_SPEED = 0.8;
/** A drag up past this far, or a flick up faster than this, opens an expanding sheet to the full screen. */
const OPEN_DRAG = 60;
const OPEN_SPEED = 0.5;

/** A sheet that rests `peek` tall and drags up to fill the screen; its content sets `full` back to false. */
export type Expand = { full: boolean; peek: number; onFull: (full: boolean) => void };

/** How far the keyboard covers the screen; Android lifts a sheet above it by itself. */
function useKeyboardHeight(): number {
  const [height, setHeight] = useState(0);
  useEffect(() => {
    if (Platform.OS !== 'ios') return;
    const show = Keyboard.addListener('keyboardWillShow', (event) => setHeight(event.endCoordinates.height));
    const hide = Keyboard.addListener('keyboardWillHide', () => setHeight(0));
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);
  return height;
}

/**
 * A sheet that slides up while the screen behind it dims in place. A tap on the
 * dimmed screen or a drag down closes it. The modal stays mounted until the sheet is down.
 * Its content scrolls when it is taller than the screen, unless `scrolls` says it scrolls itself.
 */
export function Sheet({
  visible,
  onClose,
  tall,
  scrolls,
  onDismiss,
  expand,
  footer,
  children,
}: {
  visible: boolean;
  onClose: () => void;
  tall?: boolean;
  scrolls?: boolean;
  /** Once the sheet is off the screen, as what it chose may need to show a screen of its own. */
  onDismiss?: () => void;
  expand?: Expand;
  /** Pinned to the bottom of the screen over the sheet, at whatever height the sheet rests. */
  footer?: ReactNode;
  children: ReactNode;
}) {
  const styles = useStyles(makeStyles);
  const still = useStill();
  // A modal measures no safe area of its own, so the screen behind it lends its inset.
  const insets = useSafeAreaInsets();
  const keyboard = useKeyboardHeight();
  const [mounted, setMounted] = useState(visible);
  if (visible && !mounted) setMounted(true);
  const [progress] = useState(() => new Animated.Value(0));
  const [drag] = useState(() => new Animated.Value(0));
  const [overflow, setOverflow] = useState({ content: 0, frame: 0 });
  const offscreen = Dimensions.get('window').height;
  const expandable = expand !== undefined;
  const full = expand?.full ?? false;
  const restAt = expand && !full ? offscreen - expand.peek : 0;
  const [rest] = useState(() => new Animated.Value(restAt));
  const onFull = expand?.onFull;

  const shape = useRef(expandable);
  useEffect(() => {
    // A sheet that starts or stops expanding changes its whole height, so it moves there at once.
    const jump = still || shape.current !== expandable;
    shape.current = expandable;
    if (jump) rest.setValue(restAt);
    else Animated.spring(rest, { toValue: restAt, bounciness: 0, speed: 16, useNativeDriver: true }).start();
  }, [rest, restAt, still, expandable]);

  useEffect(() => {
    if (!visible) return;
    drag.setValue(0);
    return pauseBackdrop();
  }, [visible, drag]);

  useEffect(() => {
    if (visible) {
      Animated.timing(progress, {
        toValue: 1,
        duration: still ? 0 : OPEN_MS,
        easing: Easing.out(Easing.cubic),
        useNativeDriver: true,
      }).start();
      return;
    }
    Animated.timing(progress, { toValue: 0, duration: still ? 0 : CLOSE_MS, easing: Easing.in(Easing.cubic), useNativeDriver: true }).start(
      ({ finished }) => {
        if (finished) setMounted(false);
      },
    );
  }, [visible, progress, still]);

  const [flung, setFlung] = useState(0);
  const closeFlung = useEffectEvent(onClose);
  useEffect(() => {
    if (flung) closeFlung();
  }, [flung]);
  const pan = useMemo(() => {
    const settle = () => Animated.spring(drag, { toValue: 0, bounciness: 0, useNativeDriver: true }).start();
    return PanResponder.create({
      onMoveShouldSetPanResponder: (_, { dx, dy }) => {
        if (Math.abs(dy) <= 6 || Math.abs(dy) <= Math.abs(dx)) return false;
        // A full sheet scrolls its own content, which pulls it back down from its top.
        if (full) return false;
        return expandable || dy > 0;
      },
      onPanResponderMove: (_, { dy }) => drag.setValue(Math.max(-restAt, dy)),
      onPanResponderRelease: (_, { dy, vy }) => {
        if (onFull && (dy < -OPEN_DRAG || vy < -OPEN_SPEED)) {
          rest.setValue(Math.max(0, restAt + dy));
          drag.setValue(0);
          onFull(true);
          return;
        }
        if (dy > CLOSE_DRAG || vy > CLOSE_SPEED) {
          Keyboard.dismiss();
          setFlung((count) => count + 1);
          return;
        }
        settle();
      },
      onPanResponderTerminate: settle,
    });
  }, [drag, rest, expandable, full, restAt, onFull]);

  const rise = progress.interpolate({ inputRange: [0, 1], outputRange: [offscreen, 0] });
  const handle = full ? (
    <View style={{ height: insets.top }} />
  ) : (
    <View style={styles.handle}>
      <View style={styles.grabber} />
    </View>
  );
  const bottom = keyboard ? keyboard + 12 : insets.bottom + 12;

  return (
    <Modal visible={mounted} transparent animationType="none" onRequestClose={onClose} onDismiss={onDismiss} statusBarTranslucent>
      <Animated.View style={[StyleSheet.absoluteFill, styles.scrim, { opacity: progress }]}>
        <Pressable style={{ flex: 1 }} onPress={onClose} accessibilityRole="button" accessibilityLabel="Close" />
      </Animated.View>
      <View style={[styles.dock, { paddingTop: expand ? 0 : insets.top + 24 }]} pointerEvents="box-none">
        <Animated.View
          accessibilityViewIsModal
          {...pan.panHandlers}
          style={[
            styles.sheet,
            tall && { height: '82%' },
            expand && { height: offscreen },
            { paddingBottom: bottom, transform: [{ translateY: Animated.add(Animated.add(rise, rest), drag) }] },
          ]}>
          {handle}
          {scrolls ? (
            children
          ) : (
            <ScrollView
              style={styles.content}
              showsVerticalScrollIndicator={false}
              scrollEnabled={overflow.content > overflow.frame}
              alwaysBounceVertical={false}
              keyboardShouldPersistTaps="handled"
              onLayout={(event) => {
                const frame = event.nativeEvent.layout.height;
                setOverflow((was) => (was.frame === frame ? was : { ...was, frame }));
              }}
              onContentSizeChange={(_, content) => setOverflow((was) => (was.content === content ? was : { ...was, content }))}>
              {children}
            </ScrollView>
          )}
        </Animated.View>
      </View>
      {footer ? (
        <Animated.View
          style={[styles.footer, { paddingBottom: insets.bottom + 12, transform: [{ translateY: rise }] }]}
          pointerEvents="box-none">
          {footer}
        </Animated.View>
      ) : null}
      <LockCover />
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
      maxHeight: '100%',
      backgroundColor: colors.overlay,
      borderTopLeftRadius: 22,
      borderTopRightRadius: 22,
      borderTopWidth: 1,
      borderColor: colors.border,
      paddingHorizontal: 16,
      paddingBottom: 12,
    },
    handle: { paddingTop: 8, paddingBottom: 12, alignItems: 'center' },
    grabber: { width: 36, height: 5, borderRadius: 3, backgroundColor: colors.borderStrong },
    content: { flexGrow: 0 },
    footer: { position: 'absolute', left: 0, right: 0, bottom: 0, paddingHorizontal: 16 },
    label: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary, paddingTop: 14, paddingBottom: 8, paddingHorizontal: 6 },
  });
};
