import { useEffect, useState } from 'react';
import { Pressable, StyleSheet, Text, type StyleProp, type ViewStyle } from 'react-native';
import * as Clipboard from 'expo-clipboard';

import { useT } from '@/i18n';

import { haptics } from './haptics';
import { Icon } from './Icon';
import { fonts, useColors } from './theme';

const COPIED_MS = 1200;

export function CopyButton({
  value,
  label,
  size = 13,
  style,
  worded = false,
}: {
  value: string;
  /** What the button says to a screen reader, whole: "Copy message". */
  label: string;
  size?: number;
  style?: StyleProp<ViewStyle>;
  /** Says Copy beside the icon, and Copied once it has copied, as a code block's pill does. */
  worded?: boolean;
}) {
  const t = useT();
  const colors = useColors();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), COPIED_MS);
    return () => clearTimeout(timer);
  }, [copied]);
  return (
    <Pressable
      onPress={() => {
        void Clipboard.setStringAsync(value).then(() => {
          haptics.select();
          setCopied(true);
        });
      }}
      hitSlop={10}
      style={({ pressed }) => [worded ? styles.worded : styles.button, style, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={copied ? t('Copied') : label}>
      <Icon
        name={copied ? 'IconCheck' : 'IconCopy'}
        size={size}
        color={copied ? colors.ink : worded ? colors.secondary : colors.inkFaint}
      />
      {worded ? (
        <Text style={[styles.word, { color: copied ? colors.ink : colors.secondary }]}>{copied ? t('Copied') : t('Copy')}</Text>
      ) : null}
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: { width: 26, height: 26, borderRadius: 7, alignItems: 'center', justifyContent: 'center' },
  worded: { flexDirection: 'row', alignItems: 'center', gap: 6, height: 26, paddingHorizontal: 9, borderRadius: 7 },
  word: { fontFamily: fonts.uiMedium, fontSize: 11.5 },
});
