import { StyleSheet, Switch, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { type Palette, typeFor, useColors, useStyles } from '@/ui/theme';
import { reporting, shareCrashReports, useCrashReportsShared } from './reports';

/** The account sheet's switch for sending crashes and errors to Sentry. */
export function CrashReportsRow() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const on = useCrashReportsShared();
  if (!reporting) return null;

  return (
    <View style={styles.row}>
      <Icon name="IconShield" size={18} color={colors.secondary} />
      <Text style={styles.title}>{t('Share crash reports')}</Text>
      <Switch
        value={on}
        onValueChange={shareCrashReports}
        trackColor={{ false: colors.active, true: colors.live }}
        thumbColor={colors.ink}
        accessibilityLabel={t('Share crash reports')}
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
    title: { ...type.row, flex: 1, fontSize: 15, color: colors.ink },
  });
};
