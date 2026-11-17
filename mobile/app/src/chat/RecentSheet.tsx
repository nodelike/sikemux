import { Pressable, StyleSheet, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** What this chat sent before, newest first; a tap puts one back in the composer. */
export function RecentSheet({
  visible,
  prompts,
  onClose,
  onPick,
}: {
  visible: boolean;
  prompts: readonly string[];
  onClose: () => void;
  onPick: (text: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <Sheet visible={visible} onClose={onClose}>
      <Text style={styles.sheetLabelFirst}>{t('Sent in this chat')}</Text>
      <View style={styles.recent}>
        {[...prompts]
          .reverse()
          .slice(0, RECENT_PROMPTS)
          .map((prompt, index) => (
            <Pressable
              key={index}
              onPress={() => onPick(prompt)}
              style={({ pressed }) => [styles.recentRow, pressed && { backgroundColor: colors.active }]}
              accessibilityRole="button"
              accessibilityHint={t('Puts it in the composer')}>
              <Icon name="IconClock" size={15} color={colors.inkFaint} />
              <Text style={styles.recentText} numberOfLines={2}>
                {prompt}
              </Text>
            </Pressable>
          ))}
      </View>
    </Sheet>
  );
}

const RECENT_PROMPTS = 20;

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    sheetLabelFirst: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.tertiary, paddingBottom: 8, paddingHorizontal: 6 },
    recent: { gap: 2, paddingBottom: 4 },
    recentRow: { flexDirection: 'row', alignItems: 'flex-start', gap: 12, paddingVertical: 11, paddingHorizontal: 8, borderRadius: 9 },
    recentText: { flex: 1, fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 20, color: colors.secondary },
  });
};
