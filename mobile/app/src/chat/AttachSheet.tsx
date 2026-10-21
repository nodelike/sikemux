import { memo, useRef, useState } from 'react';
import { Image, Linking, Platform, Pressable, StyleSheet, Text, useWindowDimensions, View } from 'react-native';
import { FlashList } from '@shopify/flash-list';

import { counted } from '@/devices/words';
import { Button, Chip } from '@/ui/controls';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { Sheet } from '@/ui/Sheet';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { useLibrary, type LibraryPhoto } from './library';
import { libraryPhotos, pickFiles, pickPhotos, takePhoto } from './pick';
import type { Attachment } from './uploads';

const COLUMNS = 4;
/** The space around each square in the grid. */
const INSET = 2;
const HANDLE = 25;
const TILES = 96;
const LIMITED_LINE = 30;
/** Rows of photos the resting sheet shows, the last cut through so the rest reads as more. */
const PEEK_ROWS = 2.5;
/** How far the full gallery is pulled down past its top before it drops back to the resting sheet. */
const PULL_DOWN = 70;

const CAMERA = 'camera';
/** Google Play allows reading the photo library only to apps built around it, so Android uses the system picker. */
const GALLERY = Platform.OS === 'ios';
type Cell = LibraryPhoto | typeof CAMERA;

/**
 * Where a message's photos and files come from: the camera, the newest photos in a grid that drags up
 * into the whole library, or the Files app. What is picked comes back through `onAttach`.
 */
export function AttachSheet({
  visible,
  room,
  onClose,
  onAttach,
}: {
  visible: boolean;
  /** How many more files the message takes. */
  room: number;
  onClose: () => void;
  onAttach: (picking: Promise<Attachment[]>) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const window = useWindowDimensions();
  const library = useLibrary(GALLERY && visible);
  const [full, setFull] = useState(false);
  const [chosen, setChosen] = useState<LibraryPhoto[]>([]);
  const [shown, setShown] = useState(visible);
  if (shown !== visible) {
    setShown(visible);
    if (visible) {
      setFull(false);
      setChosen([]);
    }
  }
  // iOS shows a system screen only once the sheet over the app has gone.
  const outside = useRef<(() => Promise<Attachment[]>) | null>(null);
  const leave = (pick: () => Promise<Attachment[]>) => {
    onClose();
    if (Platform.OS === 'ios') outside.current = pick;
    else onAttach(pick());
  };
  const dismissed = () => {
    const pick = outside.current;
    outside.current = null;
    if (pick) onAttach(pick());
  };

  const sees = library.access === 'all' || library.access === 'limited';
  const gallery = GALLERY && library.access !== 'denied';
  const cell = (window.width - 32) / COLUMNS;
  const peek = HANDLE + TILES + (library.access === 'limited' ? LIMITED_LINE : 0) + cell * PEEK_ROWS;
  const toggle = (photo: LibraryPhoto) => {
    haptics.select();
    setChosen((now) =>
      now.some((known) => known.id === photo.id) ? now.filter((known) => known.id !== photo.id) : now.length < room ? [...now, photo] : now,
    );
  };
  const camera = () => leave(takePhoto);
  const add = () => {
    onAttach(libraryPhotos(chosen));
    onClose();
  };

  const tiles = (
    <View style={styles.tiles}>
      <Tile icon="IconCamera" label="Camera" onPress={camera} />
      <Tile
        icon="IconImage"
        label="Photos"
        onPress={() => (sees ? setFull(true) : GALLERY && library.canAsk ? library.ask() : leave(() => pickPhotos(room)))}
      />
      <Tile icon="IconFile" label="Files" onPress={() => leave(() => pickFiles(room))} />
    </View>
  );
  const limited =
    library.access === 'limited' ? (
      <View style={styles.limited}>
        <Pressable onPress={library.manage} hitSlop={8} accessibilityRole="button">
          <Text style={styles.manage}>Manage</Text>
        </Pressable>
      </View>
    ) : null;

  return (
    <Sheet
      visible={visible}
      onClose={onClose}
      onDismiss={dismissed}
      scrolls
      expand={gallery ? { full, peek, onFull: setFull } : undefined}
      footer={
        chosen.length ? <Button kind="primary" title={`Add ${chosen.length} ${counted('photo', chosen.length)}`} onPress={add} /> : null
      }>
      {full ? (
        <View style={styles.head}>
          <Pressable onPress={onClose} hitSlop={8} style={styles.close} accessibilityRole="button" accessibilityLabel="Close">
            <Icon name="IconClose" size={15} color={colors.tertiary} />
          </Pressable>
          <Text style={styles.title}>Recents</Text>
          <View style={styles.close} />
        </View>
      ) : null}
      {gallery ? (
        <View style={styles.grid}>
          <FlashList<Cell>
            data={full ? library.photos : [CAMERA, ...library.photos]}
            numColumns={COLUMNS}
            keyExtractor={(item) => (item === CAMERA ? CAMERA : item.id)}
            extraData={chosen}
            scrollEnabled={full}
            onScrollEndDrag={(event) => {
              if (full && event.nativeEvent.contentOffset.y < -PULL_DOWN) setFull(false);
            }}
            onEndReached={library.more}
            onEndReachedThreshold={1.5}
            showsVerticalScrollIndicator={false}
            ListHeaderComponent={
              <>
                {full ? null : tiles}
                {limited}
              </>
            }
            ListFooterComponent={<View style={{ height: chosen.length ? 80 : 0 }} />}
            renderItem={({ item }) =>
              item === CAMERA ? (
                <CameraCell onPress={camera} />
              ) : (
                <PhotoCell
                  photo={item}
                  order={chosen.findIndex((known) => known.id === item.id) + 1}
                  choosing={chosen.length > 0}
                  onToggle={toggle}
                />
              )
            }
          />
        </View>
      ) : GALLERY ? (
        <>
          {tiles}
          <View style={styles.off}>
            <Text style={styles.offText}>No photo access</Text>
            {library.canAsk ? (
              <Chip primary title="Allow access" onPress={library.ask} />
            ) : (
              <Chip title="Open Settings" onPress={() => void Linking.openSettings()} />
            )}
          </View>
        </>
      ) : (
        tiles
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

function CameraCell({ onPress }: { onPress: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.cell}>
      <Pressable
        onPress={onPress}
        style={({ pressed }) => [styles.square, styles.camera, pressed && { backgroundColor: colors.active }]}
        accessibilityRole="button"
        accessibilityLabel="Take a photo">
        <Icon name="IconCamera" size={20} color={colors.ink} />
        <Text style={styles.cameraText}>Camera</Text>
      </Pressable>
    </View>
  );
}

const PhotoCell = memo(function PhotoCell({
  photo,
  order,
  choosing,
  onToggle,
}: {
  photo: LibraryPhoto;
  /** Its place among the chosen, from 1, or 0 when it is not chosen. */
  order: number;
  choosing: boolean;
  onToggle: (photo: LibraryPhoto) => void;
}) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.cell}>
      <Pressable
        onPress={() => onToggle(photo)}
        style={[styles.square, order > 0 && styles.chosen]}
        accessibilityRole="button"
        accessibilityLabel={photo.filename ?? 'Photo'}
        accessibilityState={{ selected: order > 0 }}>
        <Image source={{ uri: photo.id }} style={StyleSheet.absoluteFill} resizeMode="cover" resizeMethod="resize" fadeDuration={0} />
        {choosing ? (
          <View style={[styles.badge, order > 0 && styles.badgeOn]}>
            {order > 0 ? <Text style={styles.badgeText}>{order}</Text> : null}
          </View>
        ) : null}
      </Pressable>
    </View>
  );
});

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
    limited: { flexDirection: 'row', alignItems: 'center', justifyContent: 'flex-end', height: LIMITED_LINE, paddingHorizontal: 4 },
    manage: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.accent },
    head: { flexDirection: 'row', alignItems: 'center', height: 46, marginBottom: 6 },
    close: { width: 40, height: 40, alignItems: 'center', justifyContent: 'center' },
    title: { flex: 1, textAlign: 'center', fontFamily: fonts.uiSemibold, fontSize: 16, color: colors.ink },
    grid: { flex: 1, marginHorizontal: -INSET },
    cell: { flex: 1, aspectRatio: 1, padding: INSET },
    square: { flex: 1, borderRadius: 8, overflow: 'hidden', borderWidth: 1, borderColor: 'transparent', backgroundColor: colors.raised },
    camera: { borderColor: colors.border, alignItems: 'center', justifyContent: 'center', gap: 6 },
    cameraText: { fontFamily: fonts.uiMedium, fontSize: 12, color: colors.ink },
    chosen: { borderWidth: 2, borderColor: colors.accent },
    badge: {
      position: 'absolute',
      top: 5,
      right: 5,
      width: 22,
      height: 22,
      borderRadius: 11,
      borderWidth: 1.5,
      borderColor: 'rgba(255, 255, 255, 0.9)',
      backgroundColor: 'rgba(9, 9, 11, 0.25)',
      alignItems: 'center',
      justifyContent: 'center',
    },
    badgeOn: { top: 4, right: 4, borderColor: colors.accent, backgroundColor: colors.accent },
    badgeText: { fontFamily: fonts.uiSemibold, fontSize: 12, color: colors.ground },
    off: { flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', paddingHorizontal: 4, paddingTop: 4 },
    offText: { fontFamily: fonts.uiMedium, fontSize: 14, color: colors.secondary },
  });
