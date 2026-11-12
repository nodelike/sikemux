import { Pressable, StyleSheet, Text } from 'react-native';
import Svg, { Circle } from 'react-native-svg';

import { contextShare, contextTone, tokenCount } from '@shared/chat/contextUsage';
import type { ContextUsage } from '@shared/wire.generated';
import { agentReadsUsage, useAgentBrand } from '@shared/agents/catalog';
import { useT } from '@/i18n';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** The Mac's colour for how full the context is: the agent's own, then warm, then hot. */
export function usageColor(usage: ContextUsage, provider: string, brand: string | undefined, colors: Palette): string {
  const tone = contextTone(usage);
  if (tone === 'hot') return colors.danger;
  if (tone === 'warm') return colors.warn;
  return agentReadsUsage(provider) && brand ? brand : colors.accent;
}

const RING = 14;

const RING_STROKE = 1.8;

/**
 * How much of the context window the chat has used, as the Mac's composer rings it. A tap opens the settings sheet.
 * While the chat is to be summarised before its next message, it turns the accent and says how many tokens that covers.
 */
export function ContextRing({
  usage,
  provider,
  summarising,
  onPress,
}: {
  usage: ContextUsage;
  provider: string;
  summarising?: number;
  onPress: () => void;
}) {
  const colors = useColors();
  const brand = useAgentBrand(provider);
  const styles = useStyles(makeStyles);
  const t = useT();
  const radius = (RING - RING_STROKE) / 2;
  const around = 2 * Math.PI * radius;
  const share = contextShare(usage);
  return (
    <Pressable
      onPress={onPress}
      style={({ pressed }) => [styles.ring, summarising !== undefined && styles.wide, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={t('Context window {percent}% used', { percent: Math.round(share * 100) })}>
      <Svg width={RING} height={RING} style={{ transform: [{ rotate: '-90deg' }] }}>
        <Circle cx={RING / 2} cy={RING / 2} r={radius} stroke={colors.border} strokeWidth={RING_STROKE} fill="none" />
        <Circle
          cx={RING / 2}
          cy={RING / 2}
          r={radius}
          stroke={summarising !== undefined ? colors.accent : usageColor(usage, provider, brand, colors)}
          strokeWidth={RING_STROKE}
          strokeDasharray={`${share * around} ${around}`}
          strokeLinecap="round"
          fill="none"
        />
      </Svg>
      {summarising !== undefined ? <Text style={styles.tokens}>{tokenCount(summarising)}</Text> : null}
    </Pressable>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    ring: { minWidth: 34, height: 34, borderRadius: 17, flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 5 },
    wide: { paddingHorizontal: 10 },
    tokens: { fontFamily: fonts.mono, fontSize: 12, color: colors.accent },
  });
};
