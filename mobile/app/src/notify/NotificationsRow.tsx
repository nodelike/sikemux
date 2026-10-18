import { useState } from 'react';
import { Linking, Platform, Pressable, StyleSheet, Switch, Text, View } from 'react-native';
import { useAuth } from '@clerk/expo';

import { Icon } from '@/ui/Icon';
import { fonts, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';
import { useNotificationsChoice } from './setting';
import { notificationsSupported, turnOff, turnOn, useNotificationsAllowed } from './switch';

/** The account sheet's switch for notifications, and the way to the system's settings when it blocks them. */
export function NotificationsRow() {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const { getToken } = useAuth();
  const choice = useNotificationsChoice();
  const allowed = useNotificationsAllowed();
  const [busy, setBusy] = useState(false);
  if (!notificationsSupported) return null;
  const blocked = choice === 'on' && allowed === false;
  const on = choice === 'on' && allowed === true;

  const flip = async (next: boolean) => {
    setBusy(true);
    try {
      if (next) await turnOn(() => getToken());
      else await turnOff(() => getToken());
    } finally {
      setBusy(false);
    }
  };

  return (
    <View style={styles.row}>
      <Icon name="IconInbox" size={18} color={colors.secondary} />
      <View style={{ flex: 1 }}>
        <Text style={styles.title}>Notifications</Text>
        {blocked ? (
          <Pressable onPress={() => void Linking.openSettings()} accessibilityRole="link" hitSlop={8}>
            <Text style={[type.meta, styles.link]}>
              {Platform.OS === 'ios' ? 'Allow them in Settings' : 'Allow them in Android settings'}
            </Text>
          </Pressable>
        ) : null}
      </View>
      <Switch
        value={on}
        disabled={busy}
        onValueChange={(next) => void flip(next)}
        trackColor={{ false: colors.active, true: colors.live }}
        thumbColor={colors.ink}
        accessibilityLabel="Notifications"
      />
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      minHeight: 52,
      marginTop: 8,
      paddingHorizontal: 10,
      paddingVertical: 6,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    title: { ...type.row, fontSize: 15, color: colors.ink },
    link: { color: colors.accent, fontFamily: fonts.uiMedium },
  });
};
