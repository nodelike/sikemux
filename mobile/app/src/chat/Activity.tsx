import { useEffect, useState } from 'react';
import { Animated, Easing, StyleSheet, Text, View } from 'react-native';

import { durationLabel } from '@shared/chat/durationLabel';
import { AgentIcon } from '@/ui/Icon';
import { fonts, type Palette, useStyles } from '@/ui/theme';
import { useAnimating, useSecond } from '@/ui/motion';
import { Shimmer } from '@/ui/Shimmer';

export function Activity({ provider, label, since }: { provider: string; label: string; since: number }) {
  const styles = useStyles(makeStyles);
  const [breath] = useState(() => new Animated.Value(1));
  const animating = useAnimating();
  const [first] = useState(() => Math.round((Date.now() - since) / 1000));
  const now = useSecond(animating);
  const seconds = now ? Math.round(now - since / 1000) : first;
  useEffect(() => {
    if (!animating) return;
    const loop = Animated.loop(
      Animated.sequence([
        Animated.timing(breath, { toValue: 0.5, duration: 750, easing: Easing.inOut(Easing.ease), useNativeDriver: true }),
        Animated.timing(breath, { toValue: 1, duration: 750, easing: Easing.inOut(Easing.ease), useNativeDriver: true }),
      ]),
    );
    loop.start();
    return () => loop.stop();
  }, [breath, animating]);
  return (
    <View style={styles.activity} accessible accessibilityLabel={label}>
      <Animated.View style={{ opacity: breath }}>
        <AgentIcon provider={provider} size={20} />
      </Animated.View>
      <Shimmer style={styles.activityText} layout={styles.activityLabel}>
        {label}
      </Shimmer>
      {seconds > 0 ? <Text style={styles.activityTime}>{durationLabel(seconds * 1000)}</Text> : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    activity: { flexDirection: 'row', alignItems: 'center', gap: 9, marginTop: 14, marginBottom: 6 },
    activityText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkFaint },
    activityLabel: { flexShrink: 1 },
    activityTime: { fontFamily: fonts.mono, fontSize: 10, color: colors.inkFaint },
  });
};
