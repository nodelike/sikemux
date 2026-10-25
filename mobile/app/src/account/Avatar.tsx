import { useState } from 'react';
import { Image, PixelRatio, StyleSheet, Text, View } from 'react-native';
import { useUser } from '@clerk/expo';

import { initials } from '@shared/lib/initials';
import { fonts, type Palette, useStyles } from '@/ui/theme';

/** Clerk's image host scales the whole picture to fit the size asked for; its crop and cover fits zoom in on a slice of it. */
function sized(url: string, size: number): string {
  const pixels = PixelRatio.getPixelSizeForLayoutSize(size);
  return `${url}${url.includes('?') ? '&' : '?'}width=${pixels}&height=${pixels}&fit=scale-down&quality=100`;
}

/** The signed-in account's picture, or its initials on a neutral circle when it has none. */
export function Avatar({ size }: { size: number }) {
  const styles = useStyles(makeStyles);
  const { user } = useUser();
  const [failed, setFailed] = useState<string>();
  const url = user?.hasImage ? user.imageUrl : undefined;
  const picture = url && url !== failed ? sized(url, size) : undefined;
  const letters = user ? initials(user.fullName, user.primaryEmailAddress?.emailAddress) : '';
  return (
    <View style={[styles.circle, { width: size, height: size, borderRadius: size / 2 }]}>
      {picture ? (
        <Image source={{ uri: picture }} style={StyleSheet.absoluteFill} onError={() => setFailed(url)} accessibilityIgnoresInvertColors />
      ) : (
        <Text style={[styles.letters, { fontSize: Math.round(size * 0.4) }]} allowFontScaling={false}>
          {letters}
        </Text>
      )}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    circle: {
      alignItems: 'center',
      justifyContent: 'center',
      overflow: 'hidden',
      backgroundColor: colors.raised,
      borderWidth: 1,
      borderColor: colors.border,
    },
    letters: { fontFamily: fonts.uiSemibold, color: colors.secondary, letterSpacing: -0.2 },
  });
