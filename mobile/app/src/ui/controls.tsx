import { useState, type ReactElement } from 'react';
import {
  ActivityIndicator,
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
  type StyleProp,
  type TextInputProps,
  type TextStyle,
  type ViewStyle,
} from 'react-native';

import { haptics } from './haptics';
import { DrawnIcon, Icon } from './Icon';
import type { IconName } from './icons.generated';
import { fonts, keyboardFor, type Palette, radius, translucent, useColors, useStyles } from './theme';

export function IconButton({ name, onPress, label }: { name: Parameters<typeof Icon>[0]['name']; onPress?: () => void; label: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable style={styles.iconButton} onPress={onPress} accessibilityRole="button" accessibilityLabel={label}>
      <Icon name={name} size={18} color={colors.tertiary} />
    </Pressable>
  );
}

type ButtonKind = 'primary' | 'neutral' | 'danger' | 'text';

export function Button({
  title,
  onPress,
  kind = 'neutral',
  disabled,
  busy,
  icon,
  style,
}: {
  title: string;
  onPress?: () => void;
  kind?: ButtonKind;
  disabled?: boolean;
  /** Working on what the press started: a spinner takes the icon's place, and the button stays as bright. */
  busy?: boolean;
  /** Drawn before the title in the title's colour: one of the Mac's icons, or a sign-in provider's logo. */
  icon?: IconName | ReactElement;
  style?: StyleProp<ViewStyle>;
}) {
  const styles = useStyles(makeStyles);
  const fill = useStyles(buttonFill);
  const ink = useStyles(buttonInk);
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled || busy}
      accessibilityRole="button"
      accessibilityState={{ disabled, busy }}
      style={({ pressed }) => [styles.button, fill[kind], pressed && styles.pressed, disabled && styles.disabled, style]}>
      {busy ? (
        <ActivityIndicator size="small" color={StyleSheet.flatten(ink[kind]).color} style={styles.spinner} />
      ) : typeof icon === 'string' ? (
        <Icon name={icon} size={17} color={StyleSheet.flatten(ink[kind]).color as string} />
      ) : (
        icon
      )}
      <Text style={[styles.buttonText, ink[kind]]}>{title}</Text>
    </Pressable>
  );
}

/** A text box in the app's controls: sunken, rounded, edged in the accent while typing in it. `icon` says what goes in it. */
export function Field({ icon, ...props }: TextInputProps & { icon?: IconName }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [focused, setFocused] = useState(false);
  const input = (
    <TextInput
      placeholderTextColor={colors.tertiary}
      selectionColor={colors.accent}
      keyboardAppearance={keyboardFor(colors)}
      {...props}
      onFocus={(event) => {
        setFocused(true);
        props.onFocus?.(event);
      }}
      onBlur={(event) => {
        setFocused(false);
        props.onBlur?.(event);
      }}
      style={[styles.field, focused && styles.fieldFocused, icon && styles.fieldLed, props.style]}
    />
  );
  if (!icon) return input;
  return (
    <View>
      {input}
      <View style={styles.lead} pointerEvents="none">
        <Icon name={icon} size={18} color={focused ? colors.secondary : colors.tertiary} />
      </View>
    </View>
  );
}

export function PasswordField(props: Omit<TextInputProps, 'secureTextEntry'>) {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const [shown, setShown] = useState(false);
  return (
    <View>
      <Field icon="IconLock" {...props} secureTextEntry={!shown} style={[styles.passwordField, props.style]} />
      <Pressable
        onPress={() => setShown((was) => !was)}
        style={styles.reveal}
        accessibilityRole="button"
        accessibilityLabel={shown ? 'Hide password' : 'Show password'}
        hitSlop={6}>
        {shown ? <DrawnIcon name="eyeOff" size={18} color={colors.tertiary} /> : <Icon name="IconEye" size={18} color={colors.tertiary} />}
      </Pressable>
    </View>
  );
}

/** A small rounded control that sits in a row of others, a little apart. */
export function Chip({ title, primary, busy, onPress }: { title: string; primary?: boolean; busy?: boolean; onPress: () => void }) {
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      disabled={busy}
      onPress={() => {
        haptics.select();
        onPress();
      }}
      hitSlop={4}
      accessibilityRole="button"
      accessibilityState={{ disabled: busy, busy }}
      style={({ pressed }) => [styles.chip, primary && styles.chipPrimary, (pressed || busy) && { opacity: 0.6 }]}>
      <Text style={[styles.chipText, primary && styles.chipTextPrimary]} numberOfLines={1}>
        {title}
      </Text>
    </Pressable>
  );
}

/** The rail's scope track: two or three options, the chosen one lifted. */
export function Track<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { value: T; label: string; count?: number }[];
  value: T;
  onChange: (value: T) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.track} accessibilityRole="tablist">
      {options.map((option) => {
        const on = option.value === value;
        return (
          <Pressable
            key={option.value}
            onPress={() => onChange(option.value)}
            style={[styles.trackOption, on && styles.trackOn]}
            hitSlop={{ top: 4, bottom: 4 }}
            accessibilityRole="tab"
            accessibilityState={{ selected: on }}>
            <Text style={[styles.trackText, on && { color: colors.ink }]}>{option.label}</Text>
            {option.count ? (
              <View style={styles.count}>
                <Text style={styles.countText} maxFontSizeMultiplier={1.3}>
                  {option.count}
                </Text>
              </View>
            ) : null}
          </Pressable>
        );
      })}
    </View>
  );
}

const buttonFill = (colors: Palette): Record<ButtonKind, ViewStyle> => ({
  primary: { backgroundColor: colors.ink, borderColor: colors.ink },
  neutral: { backgroundColor: colors.raised, borderColor: colors.border },
  danger: { backgroundColor: colors.raised, borderColor: colors.border },
  text: { backgroundColor: 'transparent', borderColor: 'transparent', minHeight: 44 },
});

const buttonInk = (colors: Palette): Record<ButtonKind, TextStyle> => ({
  primary: { color: colors.ground },
  neutral: { color: colors.ink },
  danger: { color: colors.danger },
  text: { color: colors.secondary, fontFamily: fonts.uiMedium },
});

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    chip: {
      minHeight: 32,
      maxWidth: '100%',
      justifyContent: 'center',
      paddingHorizontal: 12,
      borderRadius: radius.control,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    chipPrimary: { backgroundColor: colors.ink, borderColor: colors.ink },
    chipText: { fontFamily: fonts.uiMedium, fontSize: 13.5, color: colors.ink },
    chipTextPrimary: { color: colors.ground },
    iconButton: { width: 44, height: 44, borderRadius: 22, alignItems: 'center', justifyContent: 'center' },
    button: {
      minHeight: 50,
      paddingVertical: 8,
      borderRadius: radius.row,
      borderWidth: 1,
      flexDirection: 'row',
      gap: 10,
      alignItems: 'center',
      justifyContent: 'center',
      paddingHorizontal: 16,
    },
    buttonText: { fontFamily: fonts.uiSemibold, fontSize: 16 },
    field: {
      height: 50,
      borderRadius: radius.row,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
      paddingHorizontal: 14,
      fontFamily: fonts.ui,
      fontSize: 16,
      color: colors.ink,
    },
    fieldFocused: { borderColor: colors.borderSelected },
    fieldLed: { paddingLeft: 44 },
    lead: { position: 'absolute', left: 0, top: 0, bottom: 0, width: 44, alignItems: 'center', justifyContent: 'center' },
    passwordField: { paddingRight: 48 },
    reveal: { position: 'absolute', right: 0, top: 0, bottom: 0, width: 48, alignItems: 'center', justifyContent: 'center' },
    pressed: { opacity: 0.75 },
    disabled: { opacity: 0.5 },
    spinner: { width: 18, height: 18 },
    track: {
      minHeight: 36,
      padding: 3,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: translucent(colors.raised, 0.8),
      flexDirection: 'row',
    },
    trackOption: { flex: 1, borderRadius: 7, alignItems: 'center', justifyContent: 'center', flexDirection: 'row', gap: 6 },
    trackOn: { backgroundColor: colors.active },
    trackText: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    count: {
      minWidth: 16,
      minHeight: 16,
      borderRadius: 8,
      paddingHorizontal: 5,
      backgroundColor: colors.ink,
      alignItems: 'center',
      justifyContent: 'center',
    },
    countText: { fontFamily: fonts.uiSemibold, fontSize: 11, color: colors.ground },
  });
};
