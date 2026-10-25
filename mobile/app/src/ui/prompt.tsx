import type { ReactNode } from 'react';
import { Alert, Linking, StyleSheet, Text, View, type StyleProp, type ViewStyle } from 'react-native';

import { errorText } from '@shared/lib/format';

import { DeviceIcon, Icon, type DeviceKind } from './Icon';
import type { IconName } from './icons.generated';
import { Screen, useBottomGap } from './screen';
import { type Palette, translucent, typeFor, useColors, useStyles } from './theme';

export type Glyph = IconName | DeviceKind;
type Tone = 'neutral' | 'danger';

/** Tells the person an action failed, for a screen with no line of its own to say it on. */
export function alertFailure(title: string): (error: unknown) => void {
  return (error) => Alert.alert(title, errorText(error));
}

/** Opens a link outside the app, saying so when the phone cannot. */
export function openLink(url: string) {
  Linking.openURL(url).catch(alertFailure("Couldn't open the link"));
}

function isDevice(glyph: Glyph): glyph is DeviceKind {
  return glyph === 'laptop' || glyph === 'desktop' || glyph === 'mini';
}

/** One of the Mac's icons on a raised tile: large over a prompt's title, small over an empty list's line. */
export function PromptGlyph({ icon, tone = 'neutral', small }: { icon: Glyph; tone?: Tone; small?: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const color = tone === 'danger' ? colors.danger : small ? colors.secondary : colors.ink;
  const size = small ? 20 : 34;
  return (
    <View
      style={[styles.glyph, small && styles.glyphSmall, tone === 'danger' && styles.glyphDanger]}
      accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants">
      {isDevice(icon) ? <DeviceIcon kind={icon} size={size} color={color} /> : <Icon name={icon} size={size} color={color} />}
    </View>
  );
}

/** A glyph, a title and a line under it, centred. `fill` centres it in the space it is given, for a screen of its own. */
export function Prompt({
  icon,
  tone,
  title,
  detail,
  children,
  fill,
  style,
}: {
  icon: Glyph;
  tone?: Tone;
  title: ReactNode;
  detail?: ReactNode;
  children?: ReactNode;
  fill?: boolean;
  style?: StyleProp<ViewStyle>;
}) {
  const styles = useStyles(makeStyles);
  return (
    <View style={[styles.prompt, fill && styles.fill, style]}>
      <PromptGlyph icon={icon} tone={tone} />
      <Text style={styles.title} accessibilityRole="header">
        {title}
      </Text>
      {detail ? <Text style={styles.detail}>{detail}</Text> : null}
      {children}
    </View>
  );
}

/** A screen that is only a prompt, with its buttons at the bottom. */
export function PromptScreen({ actions, ...prompt }: Parameters<typeof Prompt>[0] & { actions: ReactNode }) {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  return (
    <Screen>
      <Prompt {...prompt} fill />
      <View style={[styles.actions, { paddingBottom: bottom }]}>{actions}</View>
    </Screen>
  );
}

/** What an empty list shows: a small glyph over one quiet line. */
export function EmptyState({ icon, text }: { icon: Glyph; text: string }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.empty}>
      <PromptGlyph icon={icon} small />
      <Text style={styles.emptyText}>{text}</Text>
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    glyph: {
      width: 76,
      height: 76,
      borderRadius: 18,
      alignItems: 'center',
      justifyContent: 'center',
      borderWidth: 1,
      borderColor: colors.borderStrong,
      backgroundColor: colors.raised,
    },
    glyphSmall: { width: 44, height: 44, borderRadius: 12, borderColor: colors.border },
    glyphDanger: { borderColor: translucent(colors.danger, 0.55), backgroundColor: translucent(colors.danger, 0.08) },
    prompt: { paddingTop: 32, paddingHorizontal: 28, alignItems: 'center' },
    fill: { flex: 1, justifyContent: 'center', paddingTop: 0, paddingBottom: 120 },
    title: { ...type.title, fontSize: 22, lineHeight: 27, textAlign: 'center', marginTop: 20 },
    detail: { ...type.body, textAlign: 'center', marginTop: 8 },
    actions: { paddingHorizontal: 16, paddingTop: 12, gap: 8 },
    empty: { alignItems: 'center', gap: 12, paddingTop: 40, paddingHorizontal: 28 },
    emptyText: { ...type.meta, textAlign: 'center' },
  });
};
