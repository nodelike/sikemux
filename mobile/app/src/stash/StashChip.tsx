import { useState } from 'react';
import { Image, Pressable, StyleSheet, Text, View } from 'react-native';

import { useT } from '@/i18n';
import { AgentIcon, Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import type { StashItem } from './api';
import { Face, useBearer } from './StashPicker';
import { useStash } from './store';
import { itemTitle, type ContextChip } from './words';

export function StashPreview({ item }: { item: StashItem }) {
  const styles = useStyles(makeStyles);
  const bearer = useBearer();
  return (
    <View style={styles.preview}>
      <Face item={item} bearer={bearer} size={30} />
      <Text style={styles.title} numberOfLines={1}>
        {itemTitle(item)}
      </Text>
    </View>
  );
}

/** The stash item a file came from, while the stash still holds it. */
export function useStashItem(id: string | undefined): StashItem | undefined {
  const items = useStash().items;
  return id === undefined ? undefined : items.find((item) => item.id === id);
}

/** A link's site icon, an earlier chat's agent, or the text glyph for stashed text and a link whose site has none. */
function ChipFace({ uri }: { uri: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const host = /^https?:\/\/([^/?#]+)/i.exec(uri)?.[1];
  const earlier = /^sikemux:\/\/earlier\/([^/]+)\//.exec(uri)?.[1];
  const [broken, setBroken] = useState(false);
  return (
    <View style={styles.face}>
      {earlier ? (
        <AgentIcon provider={earlier} size={16} />
      ) : host && !broken ? (
        <Image source={{ uri: `https://${host}/favicon.ico` }} style={styles.favicon} onError={() => setBroken(true)} />
      ) : (
        <Icon name={host ? 'IconLink' : 'IconText'} size={13} color={colors.secondary} />
      )}
    </View>
  );
}

export function StashChip({ chip, onRemove }: { chip: ContextChip; onRemove: () => void }) {
  const t = useT();
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.chip}>
      <View style={styles.preview}>
        <ChipFace uri={chip.uri} />
        <Text style={styles.title} numberOfLines={1}>
          {chip.title}
        </Text>
      </View>
      <Pressable
        onPress={onRemove}
        hitSlop={10}
        style={styles.remove}
        accessibilityRole="button"
        accessibilityLabel={t('Remove {title}', { title: chip.title })}>
        <Icon name="IconClose" size={10} color={colors.ground} />
      </Pressable>
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    chip: {
      height: 44,
      maxWidth: 220,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
      overflow: 'hidden',
      justifyContent: 'center',
    },
    preview: { flexDirection: 'row', alignItems: 'center', gap: 8, paddingLeft: 6, paddingRight: 28, maxWidth: 220 },
    title: { flexShrink: 1, fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.ink },
    face: {
      width: 30,
      height: 30,
      borderRadius: 6,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
      alignItems: 'center',
      justifyContent: 'center',
    },
    favicon: { width: 16, height: 16, borderRadius: 3 },
    remove: {
      position: 'absolute',
      top: 3,
      right: 3,
      width: 18,
      height: 18,
      borderRadius: 9,
      backgroundColor: colors.ink,
      alignItems: 'center',
      justifyContent: 'center',
    },
  });
