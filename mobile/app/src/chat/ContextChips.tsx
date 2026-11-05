import { StyleSheet, Text, View } from 'react-native';

import { contextChip, type ContextChip } from '@shared/chat/promptContext';
import type { SentContext } from '@shared/chat/types';
import { Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

const CHIP_LOOKS: Record<NonNullable<ContextChip['kind']> | 'other', { label: string; icon: IconName }> = {
  issue: { label: 'Issue', icon: 'IconIssue' },
  pull: { label: 'Pull request', icon: 'IconPullRequest' },
  other: { label: 'Context', icon: 'IconFile' },
};

/** An issue, pull request or other item handed over with a message, by its number and title rather than its whole text. */
export function ContextChips({ items }: { items: SentContext[] }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.contextChips}>
      {items.map((item) => {
        const chip = contextChip(item);
        const { label, icon } = CHIP_LOOKS[chip.kind ?? 'other'];
        return (
          <View
            key={item.uri}
            style={styles.contextChip}
            accessible
            accessibilityLabel={`${label} ${chip.number ? `#${chip.number} ` : ''}${chip.title}`}>
            <Icon name={icon} size={14} color={chip.kind ? colors.live : colors.inkFaint} />
            {chip.number ? <Text style={styles.contextNumber}>#{chip.number}</Text> : null}
            <Text style={styles.contextTitle} numberOfLines={1}>
              {chip.title}
            </Text>
          </View>
        );
      })}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    contextChips: { flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'flex-end', gap: 6, maxWidth: '84%', marginBottom: 6 },
    contextChip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      maxWidth: 240,
      paddingVertical: 7,
      paddingHorizontal: 10,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    contextNumber: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.ink },
    contextTitle: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.inkDim },
  });
};
