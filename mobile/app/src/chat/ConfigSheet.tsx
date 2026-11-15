import { useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Switch, Text, View } from 'react-native';

import { contextShare, money, tokenCount } from '@shared/chat/contextUsage';
import type { ChatRows } from '@shared/chat/rows';
import { formatAgo } from '@shared/lib/format';
import type { ExpiredCache } from '@shared/wire.generated';
import { agentLabel, agentMaker, useAgentBrand } from '@shared/agents/catalog';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Field, Track } from '@/ui/controls';
import { SectionLabel } from '@/ui/list';
import { currentLabel } from '@/chat/sessionConfig';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { usageColor } from '@/chat/ContextRing';
import { type Translate, useT } from '@/i18n';

function expiredLine(cache: ExpiredCache, t: Translate): string {
  return t('Cache expired {ago} · your next message re-reads all {tokens} tokens', {
    ago: formatAgo(cache.at, Date.now(), t),
    tokens: tokenCount(cache.tokens),
  });
}

type Picker = NonNullable<ChatRows['model']>;

/** Five bars rising left to right, `lit` of them in the accent, as each step of effort shows on the Mac. */
function Strength({ lit }: { lit: number }) {
  const colors = useColors();
  return (
    <View style={{ flexDirection: 'row', alignItems: 'flex-end', gap: 1.5, height: 11 }}>
      {[0, 1, 2, 3, 4].map((bar) => (
        <View
          key={bar}
          style={{ width: 2.5, height: 3 + 2 * bar, borderRadius: 1, backgroundColor: bar < lit ? colors.accent : colors.borderStrong }}
        />
      ))}
    </View>
  );
}

/**
 * The chat's agent, model, effort and speed, picked as the agent offers them, and how full its context is. Laid out
 * as the Mac's model menu: the agents across the top, the agent's models under a search, then the effort.
 */
export function ConfigSheet({
  visible,
  onClose,
  provider,
  agents = [],
  onAgent,
  model,
  effort,
  speed,
  usage,
  expiredCache,
  onSummarise,
  onPick,
}: {
  visible: boolean;
  onClose: () => void;
  provider: string;
  /** The agents the chat can go to, in the strip across the top; it shows with more than one. */
  agents?: string[];
  onAgent?: (agent: string) => void;
  model: ChatRows['model'];
  effort: ChatRows['effort'];
  speed: ChatRows['speed'];
  usage?: ChatRows['usage'];
  expiredCache?: ExpiredCache | null;
  onSummarise?: (summarise: boolean) => void;
  onPick: (picker: Picker, value: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const brand = useAgentBrand(provider);
  const [query, setQuery] = useState('');
  const [older, setOlder] = useState(false);
  const typed = query.trim().toLowerCase();
  const name = agentLabel(provider);
  const heading = agentMaker(provider) ?? name;
  const unfolded = older || Boolean(typed);
  const folds = Boolean(model?.choices.some((choice) => choice.superseded && choice.value !== model.current));
  const shown =
    model?.choices.filter(
      (choice) =>
        (unfolded || !choice.superseded || choice.value === model.current) &&
        (!typed || `${choice.label} ${choice.description ?? ''} ${heading}`.toLowerCase().includes(typed)),
    ) ?? [];
  const steps = effort?.choices.length ?? 0;
  return (
    <Sheet visible={visible} onClose={onClose}>
      {agents.length > 1 ? (
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          contentContainerStyle={styles.strip}
          keyboardShouldPersistTaps="handled">
          {agents.map((agent) => {
            const on = agent === provider;
            return (
              <Pressable
                key={agent}
                onPress={() => (on ? undefined : onAgent?.(agent))}
                style={[styles.harness, on && { backgroundColor: colors.active }]}
                accessibilityRole="button"
                accessibilityLabel={agentLabel(agent)}
                accessibilityState={{ selected: on }}>
                <AgentIcon provider={agent} size={20} />
              </Pressable>
            );
          })}
        </ScrollView>
      ) : (
        <View style={[styles.sheetHead, { paddingBottom: 12 }]}>
          <AgentIcon provider={provider} size={20} />
          <Text style={styles.sheetTitle}>{name}</Text>
        </View>
      )}
      {model ? (
        <>
          <Field
            icon="IconSearch"
            value={query}
            onChangeText={setQuery}
            placeholder={t("Search {agent}'s models…", { agent: name })}
            autoCorrect={false}
            autoCapitalize="none"
          />
          <SectionLabel>{heading}</SectionLabel>
          {shown.map((option) => {
            const on = option.value === model.current;
            return (
              <Pressable
                key={option.value}
                onPress={() => onPick(model, option.value)}
                style={[styles.option, on && { backgroundColor: colors.active }]}
                accessibilityRole="button"
                accessibilityState={{ selected: on }}>
                <AgentIcon provider={provider} size={17} />
                <Text style={[styles.optionText, on && { color: colors.ink }]} numberOfLines={1}>
                  {option.label}
                </Text>
                {option.isDefault ? <Text style={styles.meta}>{t('Default')}</Text> : null}
                <View style={{ flex: 1 }} />
                {on ? <Icon name="IconCheck" size={15} color={colors.accent} /> : null}
              </Pressable>
            );
          })}
          {folds && !typed ? (
            <Pressable onPress={() => setOlder(!older)} style={styles.option} accessibilityRole="button">
              <View style={{ width: 17, alignItems: 'center', transform: [{ rotate: older ? '-90deg' : '0deg' }] }}>
                <Icon name="IconChevron" size={14} color={colors.tertiary} />
              </View>
              <Text style={[styles.optionText, { color: colors.tertiary }]}>{older ? t('Show less') : t('Show more')}</Text>
            </Pressable>
          ) : null}
          {!shown.length ? <Text style={styles.none}>{t('No model matches')}</Text> : null}
        </>
      ) : null}
      {typed ? null : (
        <>
          {effort && steps ? (
            <>
              <View style={styles.effortHead}>
                <Icon name="IconGauge" size={13} color={colors.tertiary} />
                <Text style={styles.label}>{t('Effort')}</Text>
                <View style={{ flex: 1 }} />
                {currentLabel(model) ? (
                  <Text style={styles.meta} numberOfLines={1}>
                    {currentLabel(model)}
                  </Text>
                ) : null}
              </View>
              <View style={styles.steps}>
                {effort.choices.map((option, at) => {
                  const on = option.value === effort.current;
                  return (
                    <Pressable
                      key={option.value}
                      onPress={() => onPick(effort, option.value)}
                      style={[styles.step, on && { backgroundColor: colors.active }]}
                      accessibilityRole="button"
                      accessibilityState={{ selected: on }}>
                      <Strength lit={Math.ceil(((at + 1) * 5) / steps)} />
                      <Text style={[styles.stepText, on && { color: colors.ink }]} numberOfLines={1} adjustsFontSizeToFit>
                        {option.label}
                      </Text>
                    </Pressable>
                  );
                })}
              </View>
            </>
          ) : null}
          {speed && speed.choices.length > 1 ? (
            <View style={styles.speed}>
              <Text style={styles.label}>{t('Speed')}</Text>
              <View style={{ width: 200 }}>
                <Track
                  value={speed.current}
                  onChange={(value) => onPick(speed, value)}
                  options={speed.choices.map((option) => ({ value: option.value, label: option.label }))}
                />
              </View>
            </View>
          ) : null}
          {usage ? (
            <View style={styles.context}>
              <View style={styles.contextTop}>
                <Text style={styles.contextLabel}>{t('Context')}</Text>
                <Text style={styles.contextValue}>
                  {Math.round(contextShare(usage) * 100)}% ·{' '}
                  {t('{used} of {size}', { used: tokenCount(usage.used), size: tokenCount(usage.size) })}
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
              {usage.compactAt ? (
                <Text style={styles.cost}>
                  {t('Compacts at {at} of {size}', { at: tokenCount(usage.compactAt), size: tokenCount(usage.size) })}
                </Text>
              ) : null}
              {usage.cost ? <Text style={styles.cost}>{t('Session cost {cost}', { cost: money(usage.cost) })}</Text> : null}
            </View>
          ) : null}
          {expiredCache ? (
            <View style={styles.expired}>
              <Text style={styles.expiredLine}>{expiredLine(expiredCache, t)}</Text>
              <View style={styles.summarise}>
                <Text style={styles.summariseLabel}>{t('Summarise the chat before your next message')}</Text>
                <Switch
                  value={expiredCache.summarise}
                  onValueChange={(on) => onSummarise?.(on)}
                  trackColor={{ false: colors.active, true: colors.accent }}
                  thumbColor={colors.ink}
                  accessibilityLabel={t('Summarise the chat before your next message')}
                />
              </View>
            </View>
          ) : null}
        </>
      )}
    </Sheet>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    cost: { marginTop: 8, fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    sheetHead: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingHorizontal: 4 },
    sheetTitle: { fontFamily: fonts.uiSemibold, fontSize: 17, letterSpacing: -0.35, color: colors.ink },
    strip: { gap: 6, paddingBottom: 10 },
    harness: { width: 38, height: 38, borderRadius: 9, alignItems: 'center', justifyContent: 'center' },
    option: { flexDirection: 'row', alignItems: 'center', gap: 11, height: 42, paddingHorizontal: 10, borderRadius: 9 },
    optionText: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 15, color: colors.secondary },
    meta: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    none: { paddingHorizontal: 10, paddingVertical: 12, fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary },
    label: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary },
    effortHead: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingTop: 14, paddingBottom: 8, paddingHorizontal: 6 },
    steps: {
      flexDirection: 'row',
      gap: 3,
      padding: 3,
      borderRadius: 14,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    step: { flex: 1, alignItems: 'center', gap: 6, paddingVertical: 9, borderRadius: 9 },
    stepText: { fontFamily: fonts.ui, fontSize: 12, color: colors.secondary },
    speed: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingTop: 14, paddingLeft: 6 },
    context: { marginTop: 20, paddingHorizontal: 4 },
    contextTop: { flexDirection: 'row', justifyContent: 'space-between' },
    contextLabel: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary },
    contextValue: { fontFamily: fonts.mono, fontSize: 12, color: colors.tertiary },
    contextBar: { marginTop: 8, height: 4, borderRadius: 2, backgroundColor: colors.border, overflow: 'hidden' },
    contextFill: { height: 4, borderRadius: 2 },
    expired: {
      marginTop: 18,
      marginHorizontal: 4,
      paddingTop: 16,
      borderTopWidth: StyleSheet.hairlineWidth,
      borderTopColor: colors.border,
    },
    expiredLine: { fontFamily: fonts.ui, fontSize: 12.5, lineHeight: 18, color: colors.secondary },
    summarise: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 48, marginTop: 8 },
    summariseLabel: { flex: 1, fontFamily: fonts.uiMedium, fontSize: 15, color: colors.ink },
  });
};
