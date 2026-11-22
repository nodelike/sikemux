import { Pressable, StyleSheet, Text } from 'react-native';

import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** The worktree an agent works in, as the host keeps it on the agent. */
export function worktreeOf(value: unknown): { branch: string; path: string } | undefined {
  if (!value || typeof value !== 'object') return undefined;
  const { branch, path } = value as { branch?: unknown; path?: unknown };
  return { branch: typeof branch === 'string' ? branch : '', path: typeof path === 'string' ? path : '' };
}

/** Whether the first message starts the chat in a new git worktree on its own branch, beside the project it works in. */
export function WorktreeSwitch({ on, disabled, onFlip }: { on: boolean; disabled: boolean; onFlip: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <Pressable
      onPress={() => {
        haptics.select();
        onFlip();
      }}
      disabled={disabled}
      hitSlop={6}
      style={({ pressed }) => [styles.switch, pressed && { backgroundColor: colors.active }, disabled && { opacity: 0.55 }]}
      accessibilityRole="switch"
      accessibilityLabel={t('Worktree')}
      accessibilityHint={t('Starts this chat in a new git worktree on its own branch')}
      accessibilityState={{ checked: on, disabled }}>
      <Icon name="IconGit" size={12} color={on ? colors.accent : colors.tertiary} />
      <Text style={[styles.switchText, on && { color: colors.ink }]}>{t('worktree')}</Text>
    </Pressable>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    switch: { flexDirection: 'row', alignItems: 'center', gap: 5, height: 26, paddingHorizontal: 8, borderRadius: 13 },
    switchText: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
  });
