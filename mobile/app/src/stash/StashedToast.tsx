import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

export function StashedToast({ done, onUndo }: { done: boolean; onUndo: () => void }) {
  const t = useT();
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.toast} accessibilityLiveRegion="polite">
      {done ? <Icon name="IconCheck" size={13} color={colors.live} /> : <ActivityIndicator size="small" color={colors.inkDim} />}
      <Text style={styles.text}>{done ? t('Stashed') : t('Stashing…')}</Text>
      {done ? (
        <Pressable
          onPress={onUndo}
          hitSlop={8}
          style={({ pressed }) => [styles.undo, pressed && { backgroundColor: colors.active }]}
          accessibilityRole="button"
          accessibilityLabel={t('Undo, put the prompt back')}>
          <Text style={styles.undoText}>{t('Undo')}</Text>
        </Pressable>
      ) : null}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    toast: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      alignSelf: 'flex-start',
      height: 36,
      marginBottom: 8,
      paddingLeft: 12,
      paddingRight: 4,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
    },
    text: { fontFamily: fonts.uiMedium, fontSize: 13, color: colors.ink, marginRight: 4 },
    undo: { height: 26, paddingHorizontal: 10, borderRadius: 7, alignItems: 'center', justifyContent: 'center' },
    undoText: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.accent },
  });
