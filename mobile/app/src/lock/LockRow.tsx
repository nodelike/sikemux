import { useState } from 'react';
import { Platform, StyleSheet, Switch, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';
import { turnLockOff, turnLockOn, useLock } from './lock';

/** The account sheet's switch for asking for Face ID, Touch ID or the passcode each time the app opens. */
export function LockRow() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const t = useT();
  const { on, asking } = useLock();
  const [problem, setProblem] = useState<string>();
  if (Platform.OS === 'web') return null;

  const flip = async (next: boolean) => {
    setProblem(undefined);
    if (next) setProblem(await turnLockOn());
    else await turnLockOff();
  };

  return (
    <View style={styles.row}>
      <Icon name="IconLock" size={18} color={colors.secondary} />
      <View style={{ flex: 1 }}>
        <Text style={styles.title}>{t('Lock Sikemux')}</Text>
        {problem ? <Text style={type.meta}>{t(problem)}</Text> : null}
      </View>
      <Switch
        value={on}
        disabled={asking}
        onValueChange={(next) => void flip(next)}
        trackColor={{ false: colors.active, true: colors.live }}
        thumbColor={colors.ink}
        accessibilityLabel={t('Lock Sikemux')}
      />
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      minHeight: 52,
      marginTop: 8,
      paddingHorizontal: 10,
      paddingVertical: 6,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    title: { ...type.row, fontSize: 15, color: colors.ink },
  });
};
