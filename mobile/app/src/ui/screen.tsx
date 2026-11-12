import { useContext, useEffect, useState, type ReactNode } from 'react';
import { Keyboard, Pressable, StyleSheet, Text, View, type StyleProp, type ViewStyle } from 'react-native';
import { SafeAreaView, useSafeAreaInsets } from 'react-native-safe-area-context';
import { useT } from '@/i18n';

import { Backdrop, BackdropContext, backdropUnderStack } from './Backdrop';
import { Icon } from './Icon';
import { goBack } from './navigate';
import { fonts, type Palette, useColors, useStyles } from './theme';

/** Space under a screen's last content: the system's home bar or gesture bar, then a little air. */
export function useBottomGap(): number {
  return useSafeAreaInsets().bottom + 12;
}

/** Whether the keyboard is up; it covers the home bar, so screens drop that gap while it is. */
export function useKeyboardShown(): boolean {
  const [shown, setShown] = useState(Keyboard.isVisible());
  useEffect(() => {
    const show = Keyboard.addListener('keyboardDidShow', () => setShown(true));
    const hide = Keyboard.addListener('keyboardDidHide', () => setShown(false));
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);
  return shown;
}

export function Screen({ children, style }: { children: ReactNode; style?: StyleProp<ViewStyle> }) {
  const styles = useStyles(makeStyles);
  const backdrop = useContext(BackdropContext);
  const underStack = backdropUnderStack && backdrop !== undefined;
  return (
    <SafeAreaView style={[styles.screen, underStack && styles.seeThrough, style]} edges={['top', 'left', 'right']}>
      {underStack ? null : <Backdrop />}
      {children}
    </SafeAreaView>
  );
}

export function Nav({ back, title, end, onBack }: { back?: string; title?: ReactNode; end?: ReactNode; onBack?: () => void }) {
  const t = useT();
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.nav}>
      <Pressable
        style={styles.back}
        onPress={onBack ?? goBack}
        hitSlop={8}
        accessibilityRole="button"
        accessibilityLabel={back ?? t('Back')}>
        <View style={styles.backChevron}>
          <Icon name="IconChevron" size={18} color={colors.secondary} />
        </View>
        {back ? <Text style={styles.backText}>{back}</Text> : null}
      </Pressable>
      <View style={styles.navTitle}>
        {typeof title === 'string' ? (
          <Text style={styles.navTitleText} numberOfLines={1}>
            {title}
          </Text>
        ) : (
          title
        )}
      </View>
      {/* An empty end still balances the back button, so a centred title stays centred. */}
      <View style={[styles.navEnd, end == null && styles.navEndEmpty]}>{end}</View>
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    screen: { flex: 1, backgroundColor: colors.ground },
    seeThrough: { backgroundColor: 'transparent' },
    nav: { minHeight: 46, flexDirection: 'row', alignItems: 'center', paddingHorizontal: 6 },
    back: { flexDirection: 'row', alignItems: 'center', minHeight: 44, paddingRight: 8, paddingLeft: 4, minWidth: 44 },
    backChevron: { transform: [{ rotate: '180deg' }] },
    backText: { fontFamily: fonts.ui, fontSize: 16, color: colors.secondary, marginLeft: 2 },
    navTitle: { flex: 1, alignItems: 'center', justifyContent: 'center', flexDirection: 'row', gap: 8 },
    navTitleText: { fontFamily: fonts.uiSemibold, fontSize: 16, letterSpacing: -0.25, color: colors.ink },
    navEnd: { flexDirection: 'row', justifyContent: 'flex-end', alignItems: 'center' },
    navEndEmpty: { minWidth: 76 },
  });
};
