import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import MaskedView from '@react-native-masked-view/masked-view';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { canSwitchMode } from '@shared/agents/catalog';
import type { ConfigPicker } from '@shared/wire.generated';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, translucent, useColors, useStyles } from '@/ui/theme';

const rainbow = (colors: Palette) => [colors.accent, colors.cmd, colors.danger, colors.warn, colors.live, colors.accent];

/** The composer's edge while the agent runs without asking: the accent at half strength, as the Mac's composer draws it. */
export const yoloEdge = (colors: Palette) => translucent(colors.accent, 0.5);

function YoloWord() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const stops = rainbow(colors);
  return (
    <MaskedView maskElement={<Text style={styles.yoloText}>yolo</Text>}>
      <Text style={[styles.yoloText, { opacity: 0 }]} importantForAccessibility="no" accessibilityElementsHidden>
        yolo
      </Text>
      <Svg style={StyleSheet.absoluteFill}>
        <Defs>
          {/* Twice the word's width, as the Mac draws it, so the word shows the first half. */}
          <LinearGradient id="yolo" x1="0" y1="0" x2="2" y2="0">
            {stops.map((color, index) => (
              <Stop key={index} offset={index / (stops.length - 1)} stopColor={color} />
            ))}
          </LinearGradient>
        </Defs>
        <Rect x="0" y="0" width="100%" height="100%" fill="url(#yolo)" />
      </Svg>
    </MaskedView>
  );
}

/**
 * YOLO or safe, as the Mac's toggle says it; a tap switches it where the host can. An agent with ways of
 * working of its own, such as OpenCode's build and plan, lists them in a menu under the two instead.
 */
export function YoloToggle({
  core,
  mode,
  provider,
  locked,
  onToggle,
  work,
  onWork,
}: {
  /** The host the agent runs on. */
  core: string;
  mode: string;
  provider: string;
  /** A turn is running or something waits on an answer, and the host refuses a switch then. */
  locked: boolean;
  onToggle: (mode: string) => void;
  work?: ConfigPicker;
  onWork?: (picker: ConfigPicker, value: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [menu, setMenu] = useState(false);
  const on = mode === 'bypass';
  const switches = canSwitchMode(provider, core);
  const working =
    work && work.choices[0] && work.choices[0].value !== work.current ? work.choices.find((one) => one.value === work.current) : undefined;
  const look = (
    <>
      <Icon name={on ? 'IconShieldBolt' : 'IconShield'} size={13} color={on ? colors.accent : colors.inkFaint} />
      {on ? <YoloWord /> : <Text style={styles.yoloText}>{t('safe')}</Text>}
      {working ? <Text style={[styles.yoloText, { color: colors.accent }]}>{working.label}</Text> : null}
    </>
  );
  if (!switches) {
    return (
      <View style={styles.yolo} accessible accessibilityLabel={on ? t('Runs without asking') : t('Asks before acting')}>
        {look}
      </View>
    );
  }
  if (work && onWork) {
    const row = (label: string, chosen: boolean, pick: () => void, about?: string | null, disabled?: boolean) => (
      <Pressable
        key={label}
        onPress={() => {
          haptics.select();
          pick();
          setMenu(false);
        }}
        disabled={disabled}
        style={[styles.option, chosen && { backgroundColor: colors.active }, disabled && { opacity: 0.55 }]}
        accessibilityRole="button"
        accessibilityState={{ selected: chosen, disabled }}>
        <View style={styles.optionWords}>
          <Text style={[styles.optionText, chosen && { color: colors.ink }]} numberOfLines={1}>
            {label}
          </Text>
          {about ? (
            <Text style={styles.optionAbout} numberOfLines={2}>
              {about}
            </Text>
          ) : null}
        </View>
        {chosen ? <Icon name="IconCheck" size={15} color={colors.accent} /> : null}
      </Pressable>
    );
    return (
      <>
        <Pressable
          onPress={() => setMenu(true)}
          style={({ pressed }) => [styles.yolo, pressed && { backgroundColor: colors.active }]}
          accessibilityRole="button"
          accessibilityLabel={t('Mode')}
          accessibilityValue={{ text: [on ? t('YOLO') : t('Safe'), working?.label].filter(Boolean).join(', ') }}>
          {look}
        </Pressable>
        <Sheet visible={menu} onClose={() => setMenu(false)}>
          {row(t('Safe'), !on, () => on && onToggle('workspace-write'), locked ? t('Changes once the turn ends') : null, locked)}
          {row(t('YOLO'), on, () => !on && onToggle('bypass'), locked ? t('Changes once the turn ends') : null, locked)}
          <View style={styles.rule} />
          {work.choices.map((choice) =>
            row(choice.label, choice.value === work.current, () => onWork(work, choice.value), choice.description),
          )}
        </Sheet>
      </>
    );
  }
  return (
    <Pressable
      onPress={() => {
        haptics.select();
        onToggle(on ? 'workspace-write' : 'bypass');
      }}
      disabled={locked}
      style={({ pressed }) => [styles.yolo, pressed && { backgroundColor: colors.active }, locked && { opacity: 0.55 }]}
      accessibilityRole="switch"
      accessibilityLabel={t('Run without asking')}
      accessibilityState={{ checked: on, disabled: locked }}
      accessibilityHint={locked ? t('Changes once the turn ends') : undefined}>
      {look}
    </Pressable>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    yolo: { flexDirection: 'row', alignItems: 'center', gap: 4, height: 34, paddingHorizontal: 8, borderRadius: 7 },
    yoloText: { fontFamily: fonts.uiSemibold, fontSize: 11, letterSpacing: 0.9, textTransform: 'uppercase', color: colors.inkFaint },
    option: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 11,
      minHeight: 42,
      paddingVertical: 6,
      paddingHorizontal: 10,
      borderRadius: 9,
    },
    optionWords: { flex: 1, gap: 2 },
    optionText: { fontFamily: fonts.ui, fontSize: 15, color: colors.secondary },
    optionAbout: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    rule: { height: StyleSheet.hairlineWidth, marginVertical: 6, marginHorizontal: 10, backgroundColor: colors.border },
  });
};
