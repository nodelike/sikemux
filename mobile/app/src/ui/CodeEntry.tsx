import { useEffect, useRef } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { useT } from '@/i18n';

import { haptics } from './haptics';
import { fonts, keyboardFor, type Palette, translucent, useColors, useStyles } from './theme';

const LENGTH = 6;

function CodeTiles({ code, state }: { code: string; state: 'typing' | 'locked' | 'failed' }) {
  const styles = useStyles(makeStyles);
  const digits = Array.from({ length: LENGTH }, (_, index) => code[index] ?? '');
  return (
    <View style={styles.tiles} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
      {digits.map((digit, index) => (
        <View
          key={index}
          style={[
            styles.tile,
            index === LENGTH / 2 && styles.half,
            state === 'typing' && index === code.length && styles.tileCurrent,
            state === 'locked' && styles.tileLocked,
            state === 'failed' && styles.tileFailed,
          ]}>
          <Text style={[styles.tileText, state === 'failed' && styles.tileTextFailed]} maxFontSizeMultiplier={1.2}>
            {digit}
          </Text>
        </View>
      ))}
    </View>
  );
}

/**
 * Six code tiles over a hidden number field, so the system keyboard fills them one by one.
 * The field stays editable while a code is checked, so the keyboard never drops; typing then is ignored.
 */
export function CodeEntry({
  code,
  onChange,
  failed,
  disabled,
}: {
  code: string;
  onChange: (code: string) => void;
  failed?: boolean;
  disabled?: boolean;
}) {
  const t = useT();
  const colors = useColors();
  const input = useRef<TextInput>(null);

  useEffect(() => {
    if (!failed) return;
    haptics.failure();
    input.current?.focus();
  }, [failed]);

  const spoken = code
    ? t('{digits}, {count} of {total} digits', { digits: code.split('').join(' '), count: code.length, total: LENGTH })
    : t('Empty');
  return (
    <Pressable
      onPress={() => input.current?.focus()}
      accessible
      accessibilityLabel={failed ? t('Code, wrong') : t('Code')}
      accessibilityValue={{ text: spoken }}
      accessibilityHint={t('Type the {count} digits', { count: LENGTH })}>
      <CodeTiles code={code} state={failed ? 'failed' : disabled ? 'locked' : 'typing'} />
      <TextInput
        ref={input}
        value={code}
        onChangeText={(text) => {
          if (!disabled) onChange(text.replace(/\D/g, '').slice(0, LENGTH));
        }}
        autoFocus
        keyboardType="number-pad"
        keyboardAppearance={keyboardFor(colors)}
        textContentType="oneTimeCode"
        autoComplete="one-time-code"
        maxLength={LENGTH}
        caretHidden
        style={hidden.field}
        accessibilityElementsHidden
        importantForAccessibility="no"
      />
    </Pressable>
  );
}

const hidden = StyleSheet.create({
  field: { position: 'absolute', opacity: 0, width: 1, height: 1 },
});

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    tiles: { flexDirection: 'row', gap: 8, justifyContent: 'center' },
    tile: {
      width: 46,
      minHeight: 58,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
      alignItems: 'center',
      justifyContent: 'center',
    },
    half: { marginLeft: 6 },
    tileCurrent: { borderColor: colors.borderSelected },
    tileLocked: { backgroundColor: colors.raised, borderColor: colors.borderStrong },
    tileFailed: { borderColor: translucent(colors.danger, 0.55), backgroundColor: translucent(colors.danger, 0.08) },
    tileText: { fontFamily: fonts.mono, fontSize: 26, color: colors.ink },
    tileTextFailed: { color: colors.danger },
  });
