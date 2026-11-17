import { useEffect, useMemo, useState } from 'react';
import { PixelRatio, StyleSheet, Text, View } from 'react-native';
import Svg, { ClipPath, Defs, G, Path, Polygon } from 'react-native-svg';

import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { ICONS } from '@/ui/icons.generated';
import { useAnimating } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** What a chat opens on before its first message, as the Mac's: Sikemux's mark in dots, asking what is being made in the project. */
export function Welcome({ project }: { project: string }) {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const t = useT();
  const [before, after = '?'] = t('What are we making in {project}?', { project: '￼' }).split('￼');
  return (
    <View style={styles.welcome}>
      <PosterMark size={96} />
      <View style={styles.greeting} accessible accessibilityLabel={t('What are we making in {project}?', { project })}>
        {before.trim() ? <Text style={styles.words}>{before.trim()}</Text> : null}
        <View style={styles.folder}>
          <Icon name="IconFolder" size={15} color={colors.live} />
        </View>
        <Text style={styles.project} numberOfLines={1} ellipsizeMode="middle">
          {project}
        </Text>
        <Text style={styles.words}>{after}</Text>
      </View>
    </View>
  );
}

/** The mark's shapes and drawing box, read from the generated icon rather than drawn again. */
const LOGO = (() => {
  const xml = ICONS.Logo;
  const box = (/viewBox="([^"]+)"/.exec(xml)?.[1] ?? '0 0 24 24').split(/\s+/).map(Number);
  const shapes = [...xml.matchAll(/<(polygon|path)\b[^>]*?\s(points|d)="([^"]+)"/g)].map((found) => ({
    kind: found[1] as 'polygon' | 'path',
    data: found[3],
  }));
  return { box, shapes };
})();

const bayer = (x: number, y: number) => {
  const a = (x ^ y) & 7;
  const b = y & 7;
  return (((a & 1) << 5) | ((b & 1) << 4) | ((a & 2) << 2) | ((b & 2) << 1) | ((a & 4) >> 1) | ((b & 4) >> 2)) / 64;
};

const wander = (x: number, y: number) =>
  0.5 + 0.25 * Math.sin(x * 0.021 + 2) * Math.cos(y * 0.017 - 2.6) + 0.25 * Math.sin((x + y) * 0.009 + 4.2);

const churn = (x: number, y: number, phase: number) =>
  0.5 + 0.25 * Math.sin(x * 0.031 + phase) * Math.cos(y * 0.027 - phase * 1.3) + 0.25 * Math.sin((x + y) * 0.016 + phase * 2.1);

/** The dots the Mac's poster mark lights at `time`, as one path of squares in points. */
function dots(size: number, scale: number, time: number): string {
  const pitch = Math.max(Math.round(2 * scale), 2);
  const dot = Math.max(Math.round(pitch * (pitch <= 4 ? 0.5 : 2 / 3)), 1);
  const cells = Math.ceil((size * scale) / pitch);
  const step = pitch / scale;
  const side = (dot / scale).toFixed(2);
  let path = '';
  for (let y = 0; y < cells; y += 1) {
    for (let x = 0; x < cells; x += 1) {
      const centreX = (x + 0.5) * step;
      const centreY = (y + 0.5) * step;
      const shareX = centreX / size;
      const shareY = centreY / size;
      const boil = churn(centreX * 0.75 + 16 * time, centreY * 0.75 - 20 * time, time * 0.4) - 0.5;
      const value = 0.82 - 0.5 * shareY + 0.12 * (wander(shareX * 580, shareY * 580) - 0.5) + 0.26 * boil;
      if (value + bayer(x, y) >= 1) path += `M${(x * step).toFixed(2)} ${(y * step).toFixed(2)}h${side}v${side}h-${side}z`;
    }
  }
  return path;
}

/** Sikemux's mark as the Mac's welcome draws it (its `poster_mark` shader): an ordered dither of the accent, its dots churning inside the mark. */
export function PosterMark({ size }: { size: number }) {
  const colors = useColors();
  const animating = useAnimating();
  const scale = PixelRatio.get();
  const [time, setTime] = useState(0);
  useEffect(() => {
    if (!animating) return;
    const start = Date.now();
    const timer = setInterval(() => setTime((Date.now() - start) / 1000), 1000 / 15);
    return () => clearInterval(timer);
  }, [animating]);
  const drawn = useMemo(() => dots(size, scale, time), [size, scale, time]);
  const [x, y, width] = LOGO.box;
  const fit = size / width;
  return (
    <View accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      <Svg width={size} height={size} viewBox={LOGO.box.join(' ')}>
        <Defs>
          <ClipPath id="poster-mark">
            {LOGO.shapes.map((shape, index) =>
              shape.kind === 'polygon' ? <Polygon key={index} points={shape.data} /> : <Path key={index} d={shape.data} />,
            )}
          </ClipPath>
        </Defs>
        <G clipPath="url(#poster-mark)">
          <G transform={`translate(${x} ${y}) scale(${1 / fit})`}>
            <Path d={drawn} fill={colors.accent} opacity={0.9} />
          </G>
        </G>
      </Svg>
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    welcome: { alignItems: 'center', gap: 24, alignSelf: 'stretch', paddingBottom: 14 },
    greeting: { flexDirection: 'row', alignItems: 'center', maxWidth: '100%' },
    words: { fontFamily: fonts.uiMedium, fontSize: 19, color: colors.ink },
    folder: { paddingLeft: 7, paddingRight: 5 },
    project: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 16, color: colors.ink },
  });
