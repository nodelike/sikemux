import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { Icon } from '@/ui/Icon';
import { fonts, type Palette, typeFor, useColors, useStyles } from '@/ui/theme';
import { chooseLanguage, LANGUAGES, type LanguageChoice, useLanguage, useLanguageChoice, useT } from '.';

/** The account sheet's choice of the app's language, named in itself, which opens in place to list them. */
export function LanguageRow() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const chosen = useLanguageChoice();
  const speaks = useLanguage();
  const [open, setOpen] = useState(false);
  const choices: { code: LanguageChoice; name: string }[] = [{ code: 'system', name: t('Same as the system') }, ...LANGUAGES];
  const current = LANGUAGES.find((language) => language.code === speaks)?.name;

  const pick = (code: LanguageChoice) => {
    setOpen(false);
    chooseLanguage(code).catch((error: unknown) => console.warn('sikemux: could not keep the language', error));
  };

  return (
    <View style={styles.box}>
      <Pressable
        onPress={() => setOpen((was) => !was)}
        style={styles.row}
        accessibilityRole="button"
        accessibilityLabel={`${t('Language')}, ${current}`}
        accessibilityState={{ expanded: open }}>
        <Icon name="IconText" size={18} color={colors.secondary} />
        <Text style={styles.title}>{t('Language')}</Text>
        <Text style={styles.current} numberOfLines={1}>
          {current}
        </Text>
      </Pressable>
      {open
        ? choices.map((option) => {
            const on = option.code === chosen;
            return (
              <Pressable
                key={option.code}
                onPress={() => pick(option.code)}
                style={({ pressed }) => [styles.option, (on || pressed) && { backgroundColor: colors.active }]}
                accessibilityRole="radio"
                accessibilityState={{ checked: on }}>
                <Text style={[styles.optionText, on && { color: colors.ink }]} numberOfLines={1}>
                  {option.name}
                </Text>
                {on ? <Icon name="IconCheck" size={17} color={colors.ink} /> : null}
              </Pressable>
            );
          })
        : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    box: {
      marginTop: 8,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
      overflow: 'hidden',
    },
    row: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 52, paddingHorizontal: 10, paddingVertical: 6 },
    title: { ...type.row, fontSize: 15, color: colors.ink, flex: 1 },
    current: { ...type.meta, fontSize: 15, flexShrink: 1 },
    option: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      minHeight: 48,
      paddingLeft: 40,
      paddingRight: 14,
      borderTopWidth: 1,
      borderTopColor: colors.border,
    },
    optionText: { flex: 1, fontFamily: fonts.uiMedium, fontSize: 15, color: colors.secondary },
  });
};
