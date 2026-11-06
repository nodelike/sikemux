import type { ReactNode } from 'react';
import { Image, Pressable, StyleSheet, Text, View } from 'react-native';

import { HAS_EXTENSION } from '@shared/chat/filePath';
import { isImagePath, localImagePath } from '@shared/lib/imagePaths';
import { basename } from '@shared/lib/paths';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { useHostFile } from './hostFiles';
import { useShowImage } from './ImageViewer';

const LOCATION = /(?::\d+){1,2}$|#L\d+(?:C\d+)?$/i;
const TWO_FOLDERS_DEEP = /^~?\/[^/\s]+\/[^/\s]/;

/**
 * The Mac asks the host whether a name it reads as a path is there before drawing it as a file.
 * `files.kinds` takes full access, which a watching phone does not have, so the phone takes only
 * names that could hardly be anything else: a folder and an extension, or a path from the root at
 * least two folders deep.
 */
export function namesFile(raw: string): boolean {
  const body = raw.replace(LOCATION, '');
  return body.includes('/') && (HAS_EXTENSION.test(body) || TWO_FOLDERS_DEEP.test(body));
}

/** Whether the phone draws `path` as a picture: anything it names as one but an SVG, which it has nothing to draw with. */
export function drawsImage(path: string): boolean {
  return isImagePath(path) && !/\.svg$/i.test(path);
}

/** A picture the phone can fetch from the host: a whole path, since it does not know which folder a chat works in. */
export function hostImagePath(raw: string): string | null {
  const path = localImagePath(raw.replace(LOCATION, ''));
  return path && drawsImage(path) ? path : null;
}

/**
 * A file the transcript names, inside a line of text: the file glyph and what the agent called it.
 * The phone cannot open a host's files in an editor, so only a picture opens, over the whole screen.
 * `link` is a name in prose, `code` one between backticks, `plain` a tool row's target.
 */
export function FileRef({ raw, label, look }: { raw: string; label: ReactNode; look: 'link' | 'code' | 'plain' }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const show = useShowImage();
  const image = hostImagePath(raw);
  return (
    <Text
      style={look === 'link' ? styles.link : look === 'code' ? styles.code : undefined}
      onPress={image ? () => show({ path: image, name: basename(image) }) : undefined}
      accessibilityRole={image ? 'imagebutton' : undefined}>
      <View style={styles.glyph}>
        <Icon name="IconFile" size={look === 'plain' ? 11 : 12} color={colors.inkDim} />
      </View>{' '}
      {label}
    </Text>
  );
}

/** A picture a message links to on the host, drawn where the link was, or the file while it cannot be. */
export function LinkedImage({ path, label }: { path: string; label: ReactNode }) {
  const styles = useStyles(makeStyles);
  const show = useShowImage();
  const uri = useHostFile(path, true);
  const name = basename(path);
  if (!uri) return <FileRef raw={path} label={label} look="link" />;
  return (
    <Pressable onPress={() => show({ uri, name })} accessibilityRole="imagebutton" accessibilityLabel={`${name}, open`}>
      <Image source={{ uri }} style={styles.picture} resizeMode="cover" accessibilityIgnoresInvertColors />
    </Pressable>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    link: { color: colors.accent },
    code: { fontFamily: fonts.mono, fontSize: 12.5, color: colors.accent, backgroundColor: colors.accentSoft },
    glyph: { transform: [{ translateY: 1.5 }] },
    picture: { width: 200, height: 150, borderRadius: 8, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.sunken },
  });
