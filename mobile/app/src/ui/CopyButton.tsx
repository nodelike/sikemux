import { useEffect, useState } from 'react';
import { Pressable, StyleSheet, type StyleProp, type ViewStyle } from 'react-native';
import * as Clipboard from 'expo-clipboard';

import { haptics } from './haptics';
import { Icon } from './Icon';
import { useColors } from './theme';

const COPIED_MS = 1200;

export function CopyButton({
  value,
  label,
  size = 13,
  style,
}: {
  value: string;
  label: string;
  size?: number;
  style?: StyleProp<ViewStyle>;
}) {
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
      style={({ pressed }) => [styles.button, style, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={copied ? 'Copied' : `Copy ${label}`}>
      <Icon name={copied ? 'IconCheck' : 'IconCopy'} size={size} color={copied ? colors.ink : colors.inkFaint} />
    </Pressable>
  );
}

const styles = StyleSheet.create({
  button: { width: 26, height: 26, borderRadius: 7, alignItems: 'center', justifyContent: 'center' },
});
