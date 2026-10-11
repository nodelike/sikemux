import { useEffect, useState, type ReactNode } from 'react';
import { Animated, Easing, StyleSheet, Text, View, type StyleProp, type TextStyle, type ViewStyle } from 'react-native';
import MaskedView from '@react-native-masked-view/masked-view';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { useAnimating, useStill } from './motion';
import { useColors } from './theme';

/** One pass of the band across the label, the Mac's live-label sweep. */
const SWEEP_MS = 2200;

/**
 * A live label with the Mac's sweep: a bright band slides across text drawn in its own colour.
 * The text is the mask, so the sweep moves on the native thread and nothing redraws per frame.
 */
export function Shimmer({
  children,
  style,
  layout,
  numberOfLines = 1,
}: {
  children: ReactNode;
  style: StyleProp<TextStyle>;
  layout?: StyleProp<ViewStyle>;
  numberOfLines?: number;
}) {
  const colors = useColors();
  const still = useStill();
  const animating = useAnimating();
  const [width, setWidth] = useState(0);
  const [sweep] = useState(() => new Animated.Value(0));
  useEffect(() => {
    if (!animating || !width) return;
    sweep.setValue(0);
    const loop = Animated.loop(Animated.timing(sweep, { toValue: 1, duration: SWEEP_MS, easing: Easing.linear, useNativeDriver: true }));
    loop.start();
    return () => loop.stop();
  }, [sweep, animating, width]);
  if (still) {
    return (
      <Text style={[style, layout as StyleProp<TextStyle>]} numberOfLines={numberOfLines}>
        {children}
      </Text>
    );
  }
  const base = (StyleSheet.flatten(style).color as string | undefined) ?? colors.inkDim;
  const band = Math.max(48, width * 0.5);
  const travel = sweep.interpolate({ inputRange: [0, 1], outputRange: [-band, width] });
  return (
    <MaskedView
      style={layout}
      maskElement={
        <Text style={style} numberOfLines={numberOfLines}>
          {children}
        </Text>
      }>
      <View onLayout={(event) => setWidth(event.nativeEvent.layout.width)}>
        <Text style={[style, styles.hidden]} numberOfLines={numberOfLines} importantForAccessibility="no" accessibilityElementsHidden>
          {children}
        </Text>
        <View style={[StyleSheet.absoluteFill, { backgroundColor: base }]} />
        <Animated.View style={[styles.band, { width: band, transform: [{ translateX: travel }] }]}>
          <Svg width={band} height="100%">
            <Defs>
              <LinearGradient id="shimmer" x1="0" y1="0" x2="1" y2="0">
                <Stop offset="0" stopColor={colors.ink} stopOpacity={0} />
                <Stop offset="0.5" stopColor={colors.ink} stopOpacity={1} />
                <Stop offset="1" stopColor={colors.ink} stopOpacity={0} />
              </LinearGradient>
            </Defs>
            <Rect x="0" y="0" width="100%" height="100%" fill="url(#shimmer)" />
          </Svg>
        </Animated.View>
      </View>
    </MaskedView>
  );
}

const styles = StyleSheet.create({
  hidden: { opacity: 0 },
  band: { position: 'absolute', top: 0, bottom: 0, left: 0 },
});
