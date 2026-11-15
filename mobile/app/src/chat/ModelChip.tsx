import { Pressable, StyleSheet, Text, View } from 'react-native';

import { agentLabel } from '@shared/agents/catalog';
import { AgentIcon, Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { useT } from '@/i18n';

/**
 * The model and its effort in one chip, as on the Mac, such as "Opus 5.5 · ◔ High"; the effort's word shortens
 * first. With neither said yet it names the agent.
 */
export function ModelChip({
  provider,
  model,
  effort,
  disabled,
  onPress,
}: {
  provider: string;
  model: string | undefined;
  effort: string | undefined;
  disabled?: boolean;
  onPress: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const named = model ?? (effort ? undefined : agentLabel(provider));
  const label = [named && t('Model, {model}', { model: named }), effort && t('Effort, {effort}', { effort })].filter(Boolean).join(', ');
  return (
    <Pressable style={styles.chip} onPress={onPress} disabled={disabled} hitSlop={5} accessibilityRole="button" accessibilityLabel={label}>
      {named ? (
        <>
          <AgentIcon provider={provider} size={16} />
          <Text style={[styles.text, styles.model]} numberOfLines={1}>
            {named}
          </Text>
        </>
      ) : null}
      {effort ? (
        <>
          {named ? <Text style={[styles.text, { color: colors.inkFaint }]}>·</Text> : null}
          <Icon name="IconGauge" size={13} color={colors.inkDim} />
          <Text style={[styles.text, styles.effort]} numberOfLines={1}>
            {effort}
          </Text>
        </>
      ) : null}
      <View style={{ transform: [{ rotate: '90deg' }], opacity: 0.6 }}>
        <Icon name="IconChevron" size={10} color={named ? colors.accent : colors.inkDim} />
      </View>
    </Pressable>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    chip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 5,
      height: 34,
      paddingHorizontal: 7,
      borderRadius: 7,
      flexShrink: 1,
      minWidth: 0,
      maxWidth: 240,
    },
    text: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    model: { color: colors.accent, flexShrink: 0, maxWidth: 140 },
    effort: { flexShrink: 1, minWidth: 0 },
  });
