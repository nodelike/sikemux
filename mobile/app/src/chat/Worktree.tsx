import { Pressable, StyleSheet, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, translucent, useColors, useStyles } from '@/ui/theme';

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

/** The line above a chat's first message saying which worktree it works in: its branch, then its folder more quietly. */
export function WorktreeNote({ branch, path }: { branch: string; path: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [before, after = ''] = t('Working in a worktree on {branch}').split('{branch}');
  return (
    <View style={styles.note} accessible accessibilityLabel={t('Working in a worktree on {branch}, at {path}', { branch, path })}>
      <View style={styles.tile}>
        <Icon name="IconGit" size={14} color={colors.accent} />
      </View>
      <View style={styles.lines}>
        <Text style={styles.line} numberOfLines={1}>
          {before}
          <Text style={styles.branch}>{branch}</Text>
          {after}
        </Text>
        <Text style={styles.path} numberOfLines={1} ellipsizeMode="head">
          {path}
        </Text>
      </View>
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    switch: { flexDirection: 'row', alignItems: 'center', gap: 5, height: 26, paddingHorizontal: 8, borderRadius: 13 },
    switchText: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    note: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingTop: 12, paddingBottom: 4 },
    tile: {
      width: 30,
      height: 30,
      borderRadius: 7,
      alignItems: 'center',
      justifyContent: 'center',
      backgroundColor: translucent(colors.accent, 0.16),
    },
    lines: { flex: 1, minWidth: 0, gap: 2 },
    line: { fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    branch: { fontFamily: fonts.mono, fontSize: 12, color: colors.ink },
    path: { fontFamily: fonts.mono, fontSize: 11.5, color: colors.inkFaint },
  });
