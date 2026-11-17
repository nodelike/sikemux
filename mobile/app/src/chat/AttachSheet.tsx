import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { StashItem } from '@/stash/api';
import { StashPicker } from '@/stash/StashPicker';
import { useT } from '@/i18n';
import { Button } from '@/ui/controls';
import { Icon } from '@/ui/Icon';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { pickFiles, pickPhotos, takePhoto } from './pick';
import type { Attachment } from './uploads';

const TILES = 96;

/**
 * Where a message's photos and files come from: the camera, the system's photo picker or its files;
 * or, opened from the composer's Stash button, the stash.
 * What is picked comes back through `onAttach`, and stash items through `onStash`.
 */
export function AttachSheet({
  visible,
  stash,
  room,
  onClose,
  onAttach,
  onStash,
  onStashPrompt,
}: {
  visible: boolean;
  /** Opens on the stash rather than the photos and files. */
  stash?: boolean;
  /** How many more files the message takes. */
  room: number;
  onClose: () => void;
  onAttach: (picking: Promise<Attachment[]>) => void;
  onStash: (items: StashItem[]) => void;
  /** Stashes what the composer holds as one prompt; absent while it holds nothing. */
  onStashPrompt?: () => void;
}) {
  const styles = useStyles(makeStyles);
  const t = useT();
  const [stashing, setStashing] = useState(false);
  const [stashChosen, setStashChosen] = useState<StashItem[]>([]);
  const [shown, setShown] = useState(visible);
  if (shown !== visible) {
    setShown(visible);
    if (visible) {
      setStashing(Boolean(stash));
      setStashChosen([]);
    }
  }
  const leave = (pick: () => Promise<Attachment[]>) => {
    onClose();
    onAttach(pick());
  };
  const toggleStash = (item: StashItem) =>
    setStashChosen((now) =>
      now.some((known) => known.id === item.id) ? now.filter((known) => known.id !== item.id) : now.length < room ? [...now, item] : now,
    );
  const addStash = () => {
    onStash(stashChosen);
    onClose();
  };

  return (
    <Sheet
      visible={visible}
      onClose={onClose}
      scrolls
      expand={stashing ? { full: true, peek: 0, onFull: (open) => open || setStashing(false) } : undefined}
      footer={
        stashing && stashChosen.length ? (
          <Button kind="primary" title={t('Add {count}', { count: stashChosen.length })} onPress={addStash} />
        ) : null
      }>
      {stashing ? (
        <StashPicker
          chosen={stashChosen}
          onToggle={toggleStash}
          onAddNew={(items) => {
            onStash(items.slice(0, room));
            onClose();
          }}
          onStashPrompt={
            onStashPrompt &&
            (() => {
              onClose();
              onStashPrompt();
            })
          }
          onBack={onClose}
          onClose={onClose}
        />
      ) : (
        <View style={styles.tiles}>
          <Tile icon="IconCamera" label={t('Camera')} onPress={() => leave(takePhoto)} />
          <Tile icon="IconImage" label={t('Photos')} onPress={() => leave(() => pickPhotos(room))} />
          <Tile icon="IconFile" label={t('Files')} onPress={() => leave(() => pickFiles(room))} />
        </View>
      )}
    </Sheet>
  );
}

function Tile({ icon, label, onPress }: { icon: 'IconCamera' | 'IconImage' | 'IconFile'; label: string; onPress: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable onPress={onPress} style={styles.tile} accessibilityRole="button" accessibilityLabel={label}>
      {({ pressed }) => (
        <>
          <View style={[styles.tileFace, pressed && { backgroundColor: colors.active }]}>
            <Icon name={icon} size={20} color={colors.ink} />
          </View>
          <Text style={styles.tileText}>{label}</Text>
        </>
      )}
    </Pressable>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    tiles: { flexDirection: 'row', gap: 10, paddingHorizontal: 4, paddingTop: 2, height: TILES },
    tile: { flex: 1, alignItems: 'center', gap: 6 },
    tileFace: {
      alignSelf: 'stretch',
      height: 54,
      borderRadius: 14,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
      alignItems: 'center',
      justifyContent: 'center',
    },
    tileText: { fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.secondary },
  });
