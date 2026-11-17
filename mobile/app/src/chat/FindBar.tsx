import { useSyncExternalStore } from 'react';
import { Pressable, StyleSheet, Text, TextInput, View } from 'react-native';

import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import type { IconName } from '@/ui/icons.generated';
import { fonts, keyboardFor, type Palette, useColors, useStyles } from '@/ui/theme';
import type { ChatFind } from './chatFind';

function Toggle({ label, name, on, onPress }: { label: string; name: string; on: boolean; onPress: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={onPress}
      hitSlop={4}
      style={({ pressed }) => [styles.button, on && styles.on, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="switch"
      accessibilityLabel={name}
      accessibilityState={{ checked: on }}>
      <Text style={[styles.toggle, on && { color: colors.ink }]}>{label}</Text>
    </Pressable>
  );
}

function Step({ icon, label, onPress }: { icon: IconName; label: string; onPress: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={onPress}
      hitSlop={4}
      style={({ pressed }) => [styles.button, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={label}>
      <Icon name={icon} size={15} color={colors.inkDim} />
    </Pressable>
  );
}

export function FindBar({ find, onClose }: { find: ChatFind; onClose: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const state = useSyncExternalStore(find.subscribe, find.snapshot);
  return (
    <View style={styles.wrap}>
      <View style={styles.bar}>
        <View style={styles.field}>
          <Icon name="IconSearch" size={14} color={colors.inkFaint} />
          <TextInput
            value={state.text}
            onChangeText={find.setText}
            autoFocus
            autoCorrect={false}
            autoCapitalize="none"
            returnKeyType="search"
            submitBehavior="submit"
            onSubmitEditing={() => find.step(true)}
            placeholder={t('Find in chat')}
            placeholderTextColor={colors.inkFaint}
            selectionColor={colors.accent}
            keyboardAppearance={keyboardFor(colors)}
            style={styles.input}
            accessibilityLabel={t('Find in chat')}
          />
          {state.label ? (
            <Text style={styles.count} accessibilityLiveRegion="polite">
              {state.label}
            </Text>
          ) : null}
        </View>
        <Toggle label="Aa" name={t('Match case')} on={state.matchCase} onPress={find.flipCase} />
        <Toggle label="W" name={t('Whole word')} on={state.wholeWord} onPress={find.flipWord} />
        <Step icon="IconArrowUp" label={t('Previous match')} onPress={() => find.step(true)} />
        <Step icon="IconArrowDown" label={t('Next match')} onPress={() => find.step(false)} />
        <Step icon="IconClose" label={t('Close find')} onPress={onClose} />
      </View>
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    wrap: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 8 },
    bar: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 2,
      padding: 5,
      borderRadius: 16,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.composer,
    },
    field: { flex: 1, minWidth: 0, flexDirection: 'row', alignItems: 'center', gap: 7, height: 34, paddingHorizontal: 8 },
    input: { flex: 1, minWidth: 0, padding: 0, fontFamily: fonts.ui, fontSize: 14.5, color: colors.ink },
    count: { fontFamily: fonts.mono, fontSize: 10.5, fontVariant: ['tabular-nums'], color: colors.inkDim },
    button: { width: 32, height: 32, borderRadius: 9, alignItems: 'center', justifyContent: 'center' },
    on: { backgroundColor: colors.selected },
    toggle: { fontFamily: fonts.uiSemibold, fontSize: 12.5, color: colors.inkDim },
  });
};
