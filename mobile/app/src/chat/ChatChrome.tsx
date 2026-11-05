import { useEffect, useRef } from 'react';
import { AccessibilityInfo, type ScrollViewProps, Pressable, StyleSheet, Text, View } from 'react-native';
import { KeyboardChatScrollView, useReanimatedKeyboardAnimation } from 'react-native-keyboard-controller';
import Animated, { useAnimatedStyle } from 'react-native-reanimated';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { type Live } from '@/devices/hub';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { pauseBackdrop } from '@/ui/motion';
import { Working } from '@/devices/AgentMarks';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

export function capitalised(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1) + (text.endsWith('.') ? '' : '.');
}

/** Above the composer while the host is out of reach: the chat stays, and what is sent waits. */
export function Reconnecting({ live, hostName, onTry }: { live: Live; hostName: string; onTry: () => void }) {
  const styles = useStyles(makeStyles);
  const closed = live.status === 'closed';
  return (
    <View style={styles.reconnect} accessibilityLiveRegion="polite">
      {closed ? null : <Working />}
      <Text style={styles.reconnectText} numberOfLines={2}>
        {closed ? `Can't reach ${hostName}. ${capitalised(live.problem)}` : `Reconnecting to ${hostName}…`}
      </Text>
      <Pressable onPress={onTry} hitSlop={10} accessibilityRole="button" accessibilityLabel={`Try reaching ${hostName} now`}>
        <Text style={styles.reconnectTry}>Try now</Text>
      </Pressable>
    </View>
  );
}

/** Something this phone asked for that did not happen, said where the person is looking. */
export function Notice({ text, onDismiss }: { text: string; onDismiss: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.notice} accessibilityLiveRegion="polite">
      <Icon name="IconWarning" size={13} color={colors.danger} />
      <Text style={styles.noticeText}>{capitalised(text)}</Text>
      <Pressable onPress={onDismiss} hitSlop={10} accessibilityRole="button" accessibilityLabel="Dismiss">
        <Icon name="IconClose" size={13} color={colors.inkDim} />
      </Pressable>
    </View>
  );
}

/** A turn ending and a request arriving are felt and, with a screen reader, heard. */
export function useArrivals(running: boolean, waiting: string | undefined, said: { done: string; asks: string }) {
  const was = useRef(running);
  const asked = useRef(waiting);
  useEffect(() => {
    if (was.current && !running) {
      haptics.success();
      AccessibilityInfo.announceForAccessibility(said.done);
    }
    was.current = running;
  }, [running, said.done]);
  // A streaming reply needs the JavaScript thread more than the moving backdrop does.
  useEffect(() => (running ? pauseBackdrop() : undefined), [running]);
  useEffect(() => {
    if (waiting && waiting !== asked.current) {
      haptics.warning();
      AccessibilityInfo.announceForAccessibility(said.asks);
    }
    asked.current = waiting;
  }, [waiting, said.asks]);
}

/** How far the transcript fades out under the header and above the composer. */
const FADE = 22;

function Fade({ id, from }: { id: string; from: 0 | 1 }) {
  return (
    <Svg width="100%" height={FADE}>
      <Defs>
        <LinearGradient id={id} x1="0" y1="0" x2="0" y2="1">
          <Stop offset="0" stopColor="#000" stopOpacity={from} />
          <Stop offset="1" stopColor="#000" stopOpacity={1 - from} />
        </LinearGradient>
      </Defs>
      <Rect x="0" y="0" width="100%" height="100%" fill={`url(#${id})`} />
    </Svg>
  );
}

/** The header and composer float over the backdrop, so the transcript fades into them rather than ending on a line. */
export function EdgeFades() {
  const { bottom } = useSafeAreaInsets();
  const { height, progress } = useReanimatedKeyboardAnimation();
  // The bottom fade rises with the composer, which rides the keyboard less the home bar it covers.
  const rise = useAnimatedStyle(() => ({ transform: [{ translateY: height.value + progress.value * bottom }] }));
  return (
    <View style={{ flex: 1 }}>
      <Fade id="transcript-top" from={0} />
      <View style={{ flex: 1, overflow: 'hidden' }}>
        <Animated.View style={[{ flex: 1 }, rise]}>
          <View style={{ flex: 1, backgroundColor: '#000' }} />
          <Fade id="transcript-bottom" from={1} />
        </Animated.View>
      </View>
    </View>
  );
}

/** The keyboard covers the home bar, so the transcript makes room for only what rises above it. */
function KeyboardScroller(props: ScrollViewProps) {
  const { bottom } = useSafeAreaInsets();
  return <KeyboardChatScrollView {...props} keyboardLiftBehavior="whenAtEnd" offset={bottom} />;
}

// Someone reading the latest keeps reading it as the keyboard rises, frame by frame with it.
export const renderScrollComponent = (props: ScrollViewProps) => <KeyboardScroller {...props} />;

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    reconnect: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      marginHorizontal: 10,
      marginTop: 8,
      paddingVertical: 9,
      paddingHorizontal: 12,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
    },
    reconnectText: { flex: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.secondary },
    reconnectTry: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.ink },
    notice: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 9,
      marginHorizontal: 10,
      marginTop: 8,
      paddingVertical: 9,
      paddingHorizontal: 12,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
    },
    noticeText: { flex: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.secondary },
  });
};
