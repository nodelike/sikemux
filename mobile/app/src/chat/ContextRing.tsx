import { Pressable, StyleSheet } from 'react-native';
import Svg, { Circle } from 'react-native-svg';

import { contextShare, contextTone } from '@shared/chat/contextUsage';
import type { ContextUsage } from '@shared/wire.generated';
import { agentReadsUsage, useAgentBrand } from '@shared/agents/catalog';
import { type Palette, useColors, useStyles } from '@/ui/theme';

/** The Mac's colour for how full the context is: the agent's own, then warm, then hot. */
export function usageColor(usage: ContextUsage, provider: string, brand: string | undefined, colors: Palette): string {
  const tone = contextTone(usage);
  if (tone === 'hot') return colors.danger;
  if (tone === 'warm') return colors.warn;
  return agentReadsUsage(provider) && brand ? brand : colors.accent;
}

const RING = 14;

const RING_STROKE = 1.8;

/** How much of the context window the chat has used, as the Mac's composer rings it. A tap opens the settings sheet. */
export function ContextRing({ usage, provider, onPress }: { usage: ContextUsage; provider: string; onPress: () => void }) {
  const colors = useColors();
  const brand = useAgentBrand(provider);
  const styles = useStyles(makeStyles);
  const radius = (RING - RING_STROKE) / 2;
  const around = 2 * Math.PI * radius;
  const share = contextShare(usage);
  return (
    <Pressable
      onPress={onPress}
      style={({ pressed }) => [styles.ring, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={`Context window ${Math.round(share * 100)}% used`}>
      <Svg width={RING} height={RING} style={{ transform: [{ rotate: '-90deg' }] }}>
        <Circle cx={RING / 2} cy={RING / 2} r={radius} stroke={colors.border} strokeWidth={RING_STROKE} fill="none" />
        <Circle
          cx={RING / 2}
          cy={RING / 2}
          r={radius}
          stroke={usageColor(usage, provider, brand, colors)}
          strokeWidth={RING_STROKE}
          strokeDasharray={`${share * around} ${around}`}
          strokeLinecap="round"
          fill="none"
        />
      </Svg>
    </Pressable>
  );
}

const makeStyles = () => {
  return StyleSheet.create({
    ring: { width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center' },
  });
};
