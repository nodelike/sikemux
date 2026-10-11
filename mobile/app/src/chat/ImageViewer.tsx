import { useState } from 'react';
import { ActivityIndicator, Image, Modal, Platform, Pressable, ScrollView, Share, StyleSheet, Text, View } from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';
import { Directory, File } from 'expo-file-system';

import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, radius, useColors, useStyles } from '@/ui/theme';

const MIME: Record<string, string> = {
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  png: 'image/png',
  gif: 'image/gif',
  webp: 'image/webp',
  heic: 'image/heic',
  heif: 'image/heif',
  bmp: 'image/bmp',
  tif: 'image/tiff',
  tiff: 'image/tiff',
};

/** iOS's share sheet carries Save Image and Save to Files; Android saves into a folder the person picks, needing no storage access. */
async function save(uri: string, name: string): Promise<boolean> {
  if (Platform.OS === 'ios') {
    const result = await Share.share({ url: uri });
    return result.action === Share.sharedAction;
  }
  const folder = await Directory.pickDirectoryAsync();
  const extension = name.slice(name.lastIndexOf('.') + 1).toLowerCase();
  const copy = folder.createFile(name, MIME[extension] ?? 'application/octet-stream');
  copy.write(await new File(uri).bytes());
  return true;
}

/** A picture a message carried, over the whole screen, to look at closely and save. */
export function ImageViewer({ uri, name, onClose }: { uri: string | undefined; name: string; onClose: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const insets = useSafeAreaInsets();
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const onSave = () => {
    if (!uri || saving) return;
    setSaving(true);
    setProblem(null);
    save(uri, name)
      .then((done) => {
        if (done) haptics.success();
      })
      .catch((error: unknown) => {
        const message = error instanceof Error ? error.message : String(error);
        if (!/cancel/i.test(message)) setProblem(`Could not save it: ${message}`);
      })
      .finally(() => setSaving(false));
  };

  return (
    <Modal visible transparent animationType="fade" onRequestClose={onClose} statusBarTranslucent navigationBarTranslucent>
      <View style={styles.ground} accessibilityViewIsModal>
        <ScrollView
          style={StyleSheet.absoluteFill}
          contentContainerStyle={styles.stage}
          maximumZoomScale={5}
          minimumZoomScale={1}
          centerContent
          showsHorizontalScrollIndicator={false}
          showsVerticalScrollIndicator={false}>
          {uri ? (
            <Image source={{ uri }} style={styles.image} resizeMode="contain" accessibilityLabel={name} accessibilityIgnoresInvertColors />
          ) : (
            <ActivityIndicator color={colors.ink} />
          )}
        </ScrollView>
        <View style={[styles.bar, { paddingTop: insets.top + 8 }]}>
          <Pressable onPress={onClose} hitSlop={10} style={styles.control} accessibilityRole="button" accessibilityLabel="Close">
            <Icon name="IconClose" size={16} color={colors.ink} />
          </Pressable>
          <Text style={styles.name} numberOfLines={1} ellipsizeMode="middle">
            {name}
          </Text>
          <Pressable
            onPress={onSave}
            disabled={!uri || saving}
            hitSlop={10}
            style={[styles.control, (!uri || saving) && { opacity: 0.5 }]}
            accessibilityRole="button"
            accessibilityLabel={`Save ${name}`}>
            {saving ? <ActivityIndicator size="small" color={colors.ink} /> : <Icon name="IconDownload" size={16} color={colors.ink} />}
          </Pressable>
        </View>
        {problem ? (
          <Text style={[styles.problem, { bottom: insets.bottom + 16 }]} numberOfLines={2}>
            {problem}
          </Text>
        ) : null}
      </View>
    </Modal>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    ground: { flex: 1, backgroundColor: 'rgba(0, 0, 0, 0.94)' },
    stage: { flexGrow: 1, alignItems: 'center', justifyContent: 'center' },
    image: { width: '100%', height: '100%' },
    bar: {
      position: 'absolute',
      top: 0,
      left: 0,
      right: 0,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      paddingHorizontal: 16,
    },
    control: {
      width: 36,
      height: 36,
      borderRadius: radius.control,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
      alignItems: 'center',
      justifyContent: 'center',
    },
    name: { flex: 1, fontFamily: fonts.mono, fontSize: 12, color: colors.inkDim, textAlign: 'center' },
    problem: {
      position: 'absolute',
      left: 16,
      right: 16,
      fontFamily: fonts.ui,
      fontSize: 12,
      lineHeight: 16,
      color: colors.danger,
      textAlign: 'center',
    },
  });
};
