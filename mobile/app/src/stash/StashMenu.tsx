import { Pressable, StyleSheet, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { fonts, type Palette, useStyles } from '@/ui/theme';
import type { StashItem } from './api';
import { Face, useBearer } from './StashPicker';
import { useStash } from './store';
import { itemPlace, itemTitle, stashMatches } from './words';

const SHOWN = 4;

/** The stash items an `&` word in the composer names, above the composer as its slash commands show; a tap attaches one. */
export function StashMenu({ query, onPick }: { query: string; onPick: (item: StashItem) => void }) {
  const t = useT();
  const styles = useStyles(makeStyles);
  const stash = useStash();
  const bearer = useBearer();
  const matches = stashMatches(stash.items, query).slice(0, SHOWN);
  if (!matches.length) {
    return (
      <View style={styles.menu}>
        <View style={[styles.item, styles.none]}>
          <Text style={styles.place}>
            {stash.items.length
              ? t('Nothing in the stash matches {query}', { query })
              : t('Nothing stashed. Share to Sikemux from any app to keep it here.')}
          </Text>
        </View>
      </View>
    );
  }
  return (
    <View style={styles.menu} accessibilityRole="menu">
      {matches.map((item) => (
        <Pressable
          key={item.id}
          onPress={() => {
            haptics.select();
            onPick(item);
          }}
          style={({ pressed }) => [styles.item, pressed && { opacity: 0.8 }]}
          accessibilityRole="menuitem"
          accessibilityLabel={t('{title}, from the stash', { title: itemTitle(item) })}>
          <Face item={item} bearer={bearer} size={34} />
          <View style={styles.body}>
            <Text style={styles.title} numberOfLines={1}>
              {itemTitle(item)}
            </Text>
            <Text style={styles.place} numberOfLines={1}>
              {item.note || itemPlace(item, t)}
            </Text>
          </View>
        </Pressable>
      ))}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    menu: { gap: 4, marginBottom: 8 },
    item: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      minHeight: 48,
      paddingHorizontal: 8,
      paddingVertical: 5,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
    },
    none: { minHeight: 40, paddingHorizontal: 12 },
    body: { flex: 1, minWidth: 0 },
    title: { fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink },
    place: { marginTop: 1, fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
  });
