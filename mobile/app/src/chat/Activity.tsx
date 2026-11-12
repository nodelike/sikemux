import { useEffect, useState } from 'react';
import { Animated, Easing, StyleSheet, Text, View } from 'react-native';

import type { Doing } from '@shared/chat/chatStatus';
import { durationLabel } from '@shared/chat/durationLabel';
import { useT } from '@/i18n';
import { AgentIcon } from '@/ui/Icon';
import { fonts, type Palette, useStyles } from '@/ui/theme';
import { useAnimating, useSecond } from '@/ui/motion';
import { Shimmer } from '@/ui/Shimmer';

/** The line under the last row: the agent's mark, a sweep on what it is doing, then what it is on and for how long. */
export function Activity({ provider, doing, since }: { provider: string; doing: Doing; since: number }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  const verb = t(doing.verb);
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
    <View style={styles.activity} accessible accessibilityLabel={doing.what ? `${verb} ${doing.what}` : verb}>
      <Animated.View style={{ opacity: breath }}>
        <AgentIcon provider={provider} size={20} />
      </Animated.View>
      <Shimmer style={styles.verb}>{verb}</Shimmer>
      {doing.what ? (
        <Text style={[styles.what, doing.typed && styles.typed]} numberOfLines={1}>
          {doing.what}
        </Text>
      ) : null}
      {seconds > 0 ? <Text style={styles.time}>{durationLabel(seconds * 1000)}</Text> : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    activity: { flexDirection: 'row', alignItems: 'center', gap: 8, marginTop: 14, marginBottom: 6 },
    verb: { fontFamily: fonts.uiMedium, fontSize: 13, color: colors.inkDim },
    what: { flexShrink: 1, minWidth: 0, fontFamily: fonts.ui, fontSize: 12.5, color: colors.secondary },
    typed: { fontFamily: fonts.mono, fontSize: 12 },
    time: { flexShrink: 0, fontFamily: fonts.mono, fontSize: 10.5, color: colors.inkFaint },
  });
};
