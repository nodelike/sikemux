import { useState, type Ref } from 'react';
import { Platform, StyleSheet, Text, TextInput, View, type TextInputProps } from 'react-native';

import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

type Props = Omit<TextInputProps, 'style' | 'placeholder'> & { placeholder: string; ref?: Ref<TextInput> };

/**
 * The chat composer's text field. Android sizes a field by its text alone, so an empty one
 * would cut off a placeholder that wraps; a hidden copy of the placeholder sets its height.
 */
export function ComposerInput({ placeholder, value, ref, ...props }: Props) {
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
        value={value}
        placeholder={placeholder}
        placeholderTextColor={colors.inkFaint}
        multiline
        selectionColor={colors.accent}
        keyboardAppearance="dark"
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
      fontSize: 15,
      color: colors.ink,
      // Android pads a field by the font's full height and adds line height above each line, so the cursor misses the text.
      ...Platform.select({ ios: { lineHeight: 21 }, android: { includeFontPadding: false, textAlignVertical: 'top' } }),
    },
    measure: { position: 'absolute', left: 0, right: 0, opacity: 0 },
  });
};
