import { useState, type Ref } from 'react';
import { Platform, StyleSheet, Text, TextInput, View, type TextInputProps } from 'react-native';

import { fonts, isLight, type Palette, useColors, useStyles } from '@/ui/theme';
import { IMAGE } from './imageTags';
import type { DraftToken } from './mentions';

type Props = Omit<TextInputProps, 'style' | 'placeholder'> & {
  placeholder: string;
  ref?: Ref<TextInput>;
  /** The chips in the text, drawn apart from the words around them where the field can style its text, which the web's cannot. */
  tokens?: readonly DraftToken[];
};

function withChips(text: string, tokens: readonly DraftToken[], chip: object, picture: object) {
  const spans = [];
  let at = 0;
  for (const token of [...tokens].sort((a, b) => a.start - b.start)) {
    if (token.start > at) spans.push(text.slice(at, token.start));
    spans.push(
      <Text key={token.start} style={token.id.startsWith(IMAGE) ? picture : chip}>
        {text.slice(token.start, token.end)}
      </Text>,
    );
    at = token.end;
  }
  if (at < text.length) spans.push(text.slice(at));
  return spans;
}

/**
 * The chat composer's text field. Android sizes a field by its text alone, so an empty one
 * would cut off a placeholder that wraps; a hidden copy of the placeholder sets its height.
 */
export function ComposerInput({ placeholder, value, ref, tokens, ...props }: Props) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [placeholderHeight, setPlaceholderHeight] = useState(0);
  const empty = !value;
  return (
    <View>
      {empty ? (
        <Text
          style={[styles.input, styles.measure]}
          onLayout={(event) => setPlaceholderHeight(event.nativeEvent.layout.height)}
          importantForAccessibility="no-hide-descendants"
          accessibilityElementsHidden>
          {placeholder}
        </Text>
      ) : null}
      <TextInput
        ref={ref}
        {...(tokens?.length && value && Platform.OS !== 'web'
          ? { children: withChips(value, tokens, styles.chip, styles.picture) }
          : { value })}
        placeholder={placeholder}
        placeholderTextColor={colors.inkFaint}
        multiline
        selectionColor={colors.accent}
        keyboardAppearance={isLight(colors) ? 'light' : 'dark'}
        style={[styles.input, empty && { minHeight: Math.max(MIN_HEIGHT, placeholderHeight) }]}
        {...props}
      />
    </View>
  );
}

const MIN_HEIGHT = 44;

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    input: {
      minHeight: MIN_HEIGHT,
      maxHeight: 160,
      paddingHorizontal: 10,
      paddingTop: 9,
      paddingBottom: 4,
      fontFamily: fonts.ui,
      fontSize: 14.5,
      color: colors.ink,
      // Android pads a field by the font's full height and adds line height above each line, so the cursor misses the text.
      ...Platform.select({ android: { includeFontPadding: false, textAlignVertical: 'top' } }),
    },
    measure: { position: 'absolute', left: 0, right: 0, opacity: 0 },
    chip: { color: colors.accent, backgroundColor: colors.active },
    // A picture's tag reads as the Mac's chip for it: full ink on the chosen grey.
    picture: { color: colors.ink, backgroundColor: colors.active },
  });
};
