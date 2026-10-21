import { useState } from 'react';
import { ActivityIndicator, Image, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { basename } from '@mac/lib/paths';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { useHostFile } from './hostFiles';
import { ImageViewer } from './ImageViewer';
import { sizeLabel } from './pick';
import type { Attachment } from './uploads';

const IMAGE_PATH = /\.(jpe?g|png|gif|webp|heic|heif|bmp|tiff?)$/i;

function FileChip({ name, size, image, tall }: { name: string; size?: number; image?: boolean; tall?: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={[styles.chip, tall && styles.chipTall]}>
      <Icon name={image ? 'IconImage' : 'IconFile'} size={16} color={colors.inkDim} />
      <View style={styles.chipWords}>
        <Text style={styles.chipName} numberOfLines={1} ellipsizeMode="middle">
          {name}
        </Text>
        {size !== undefined ? <Text style={styles.chipSize}>{sizeLabel(size)}</Text> : null}
      </View>
    </View>
  );
}

/** What is picked for the next message, above the composer's text, each with its upload's state. */
export function ComposerAttachments({
  attachments,
  onRemove,
  onRetry,
}: {
  attachments: readonly Attachment[];
  onRemove: (id: string) => void;
  onRetry: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const failed = attachments.filter((attachment) => attachment.upload === 'failed');
  return (
    <View>
      <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.row} keyboardShouldPersistTaps="handled">
        {attachments.map((attachment) => {
          const sending = attachment.upload === 'sending';
          const broken = attachment.upload === 'failed';
          return (
            <Pressable
              key={attachment.id}
              disabled={!broken}
              onPress={onRetry}
              style={[styles.item, attachment.kind === 'image' ? styles.thumb : styles.fileItem, broken && { borderColor: colors.danger }]}
              // A tile that can be taken off leaves its × reachable as a button of its own.
              accessible={broken}
              accessibilityRole={broken ? 'button' : undefined}
              accessibilityLabel={
                broken
                  ? `${attachment.name} did not reach the host${attachment.problem ? `: ${attachment.problem}` : ''}. Try again`
                  : undefined
              }>
              <View style={styles.tileBody} accessible={!broken} accessibilityLabel={`${attachment.name}${sending ? ', sending' : ''}`}>
                {attachment.kind === 'image' ? (
                  <Image source={{ uri: attachment.uri }} style={styles.thumbImage} resizeMode="cover" />
                ) : (
                  <FileChip name={attachment.name} size={attachment.size} tall />
                )}
              </View>
              {sending || broken ? (
                <View style={styles.veil}>
                  {sending ? <ActivityIndicator color={colors.ink} /> : <Icon name="IconRefresh" size={18} color={colors.ink} />}
                </View>
              ) : (
                <Pressable
                  onPress={() => onRemove(attachment.id)}
                  hitSlop={10}
                  style={styles.remove}
                  accessibilityRole="button"
                  accessibilityLabel={`Remove ${attachment.name}`}>
                  <Icon name="IconClose" size={10} color={colors.ground} />
                </Pressable>
              )}
            </Pressable>
          );
        })}
      </ScrollView>
      {failed.length ? (
        <Text style={styles.problem} numberOfLines={2}>
          {failed.length === 1 ? `${failed[0].name} did not reach the host` : `${failed.length} files did not reach the host`}
          {failed[0].problem ? `: ${failed[0].problem}` : ''}. Tap to try again.
        </Text>
      ) : null}
    </View>
  );
}

/** A picture a message carried: a thumbnail once it is on this phone, and the picture over the whole screen on a tap. */
function SentImage({ path, picked }: { path: string; picked?: Attachment }) {
  const styles = useStyles(makeStyles);
  const [open, setOpen] = useState(false);
  const fetched = useHostFile(path, !picked);
  const uri = picked?.uri ?? fetched;
  const name = picked?.name ?? basename(path);
  return (
    <>
      <Pressable onPress={() => setOpen(true)} accessibilityRole="imagebutton" accessibilityLabel={`${name}, open`}>
        {uri ? <Image source={{ uri }} style={styles.sentThumb} resizeMode="cover" /> : <FileChip name={name} size={picked?.size} image />}
      </Pressable>
      {open ? <ImageViewer uri={uri} name={name} onClose={() => setOpen(false)} /> : null}
    </>
  );
}

/** The files a message carried, as this phone picked them or as the host keeps them. */
export function SentAttachments({ paths, sentFiles }: { paths: readonly string[]; sentFiles: ReadonlyMap<string, Attachment> }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.sent}>
      {paths.map((path) => {
        const picked = sentFiles.get(path);
        if (picked ? picked.kind === 'image' : IMAGE_PATH.test(path)) return <SentImage key={path} path={path} picked={picked} />;
        return (
          <View key={path} accessible accessibilityLabel={`File ${basename(path)}`}>
            <FileChip name={picked?.name ?? basename(path)} size={picked?.size} />
          </View>
        );
      })}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    row: { gap: 7, paddingHorizontal: 4, paddingTop: 4, paddingBottom: 2 },
    item: { height: 44, borderRadius: 9, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.sunken, overflow: 'hidden' },
    thumb: { width: 58 },
    tileBody: { flex: 1, justifyContent: 'center' },
    thumbImage: { width: '100%', height: '100%' },
    fileItem: { maxWidth: 190, justifyContent: 'center' },
    veil: { ...StyleSheet.absoluteFill, alignItems: 'center', justifyContent: 'center', backgroundColor: 'rgba(9, 9, 11, 0.55)' },
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
    problem: { fontFamily: fonts.ui, fontSize: 12, lineHeight: 16, color: colors.danger, paddingHorizontal: 6, paddingTop: 6 },
    chip: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      maxWidth: 220,
      paddingVertical: 7,
      paddingHorizontal: 10,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    chipTall: { height: '100%', maxWidth: undefined, paddingRight: 26, borderWidth: 0, borderRadius: 0, backgroundColor: 'transparent' },
    chipWords: { flexShrink: 1 },
    chipName: { fontFamily: fonts.mono, fontSize: 12, color: colors.ink },
    chipSize: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkFaint, marginTop: 2 },
    sent: {
      flexDirection: 'row',
      flexWrap: 'wrap',
      alignItems: 'flex-end',
      justifyContent: 'flex-end',
      gap: 6,
      maxWidth: '84%',
      marginBottom: 6,
    },
    sentThumb: { width: 132, height: 99, borderRadius: 10, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.sunken },
  });
};
