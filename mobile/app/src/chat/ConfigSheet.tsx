import { Pressable, StyleSheet, Text, View } from 'react-native';

import { contextShare, money, tokenCount } from '@shared/chat/contextUsage';
import type { ChatRows } from '@shared/chat/rows';
import { agentLabel, useAgentBrand } from '@shared/agents/catalog';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Track } from '@/ui/controls';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { usageColor } from '@/chat/ContextRing';

export function ConfigSheet({
  visible,
  onClose,
  provider,
  model,
  effort,
  usage,
  onPick,
}: {
  visible: boolean;
  onClose: () => void;
  provider: string;
  model: ChatRows['model'];
  effort: ChatRows['effort'];
  usage: ChatRows['usage'];
  onPick: (picker: NonNullable<ChatRows['model']>, value: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const brand = useAgentBrand(provider);
  return (
    <Sheet visible={visible} onClose={onClose}>
      <View style={styles.sheetHead}>
        <AgentIcon provider={provider} size={20} />
        <Text style={styles.sheetTitle}>{agentLabel(provider)}</Text>
      </View>
      {model ? (
        <>
          <Text style={styles.sheetLabel}>Model</Text>
          <View style={styles.group}>
            {model.choices.map((option, index) => {
              const on = option.value === model.current;
              return (
                <Pressable
                  key={option.value}
                  onPress={() => onPick(model, option.value)}
                  style={[styles.option, on && { backgroundColor: colors.active }, index > 0 && styles.optionDivided]}>
                  <AgentIcon provider={provider} size={18} />
                  <Text style={[styles.optionText, on && { color: colors.ink }]} numberOfLines={1}>
                    {option.label}
                  </Text>
                  {on ? <Icon name="IconCheck" size={17} color={colors.ink} /> : null}
                </Pressable>
              );
            })}
          </View>
        </>
      ) : null}
      {effort ? (
        <>
          <Text style={styles.sheetLabel}>Effort</Text>
          <Track
            value={effort.current}
            onChange={(value) => onPick(effort, value)}
            options={effort.choices.map((option) => ({ value: option.value, label: option.label }))}
          />
        </>
      ) : null}
      {usage ? (
        <View style={styles.context}>
          <View style={styles.contextTop}>
            <Text style={styles.contextLabel}>Context</Text>
            <Text style={styles.contextValue}>
              {Math.round(contextShare(usage) * 100)}% · {tokenCount(usage.used)} of {tokenCount(usage.size)}
            </Text>
          </View>
          <View style={styles.contextBar}>
            <View
              style={[
                styles.contextFill,
                { width: `${contextShare(usage) * 100}%`, backgroundColor: usageColor(usage, provider, brand, colors) },
              ]}
            />
          </View>
          {usage.cost ? <Text style={styles.cost}>Session cost {money(usage.cost)}</Text> : null}
        </View>
      ) : null}
    </Sheet>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    cost: { marginTop: 8, fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    sheetHead: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingHorizontal: 4 },
    sheetTitle: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    sheetLabel: {
      fontFamily: fonts.uiSemibold,
      fontSize: 13,
      color: colors.tertiary,
      paddingTop: 18,
      paddingBottom: 8,
      paddingHorizontal: 6,
    },
    group: { borderRadius: 14, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.raised, overflow: 'hidden' },
    option: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 52, paddingHorizontal: 14 },
    optionDivided: { borderTopWidth: 1, borderTopColor: colors.border },
    optionText: { flex: 1, fontFamily: fonts.uiMedium, fontSize: 15.5, color: colors.secondary },
    context: { marginTop: 20, paddingHorizontal: 4 },
    contextTop: { flexDirection: 'row', justifyContent: 'space-between' },
    contextLabel: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary },
    contextValue: { fontFamily: fonts.mono, fontSize: 12, color: colors.tertiary },
    contextBar: { marginTop: 8, height: 4, borderRadius: 2, backgroundColor: colors.border, overflow: 'hidden' },
    contextFill: { height: 4, borderRadius: 2 },
  });
};
