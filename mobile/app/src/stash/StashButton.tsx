import { Pressable, StyleSheet, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { newInStash, useStash } from './store';

/** The composer's way into the stash, counting what came in since the person last looked. */
export function StashButton({ onPress }: { onPress: () => void }) {
  const t = useT();
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const fresh = newInStash(useStash()).length;
  return (
    <Pressable
      onPress={onPress}
      style={({ pressed }) => [styles.button, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={fresh ? t('Stash, {count} new', { count: fresh }) : t('Stash')}>
      <Icon name="IconStack" size={16} color={fresh ? colors.ink : colors.inkDim} />
      {fresh ? (
        <View style={styles.count}>
          <Text style={styles.countText}>{fresh > 99 ? '99+' : fresh}</Text>
        </View>
      ) : null}
    </Pressable>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    button: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 5,
      height: 34,
      minWidth: 34,
      paddingHorizontal: 8,
      borderRadius: 7,
      justifyContent: 'center',
    },
    count: {
      minWidth: 17,
      height: 16,
      paddingHorizontal: 5,
      borderRadius: 8,
      backgroundColor: colors.accent,
      alignItems: 'center',
      justifyContent: 'center',
    },
    countText: { fontFamily: fonts.uiSemibold, fontSize: 10.5, color: colors.ground, fontVariant: ['tabular-nums'] },
  });
