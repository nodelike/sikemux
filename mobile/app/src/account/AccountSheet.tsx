import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { useAuth, useUser } from '@clerk/expo';
import { nativeApplicationVersion, nativeBuildVersion } from 'expo-application';
import * as Updates from 'expo-updates';

import { ApiProblem, removePhone } from '@/account/api';
import { Avatar } from '@/account/Avatar';
import { providerName } from '@/account/providers';
import { signOutHere } from '@/account/leave';
import { retryRegistration, useAccountStatus } from '@/account/session';
import { versionLabel } from '@/account/versionLabel';
import { useDeviceId } from '@/device/identity';
import { shortKey } from '@/devices/paired';
import { versionFromBuild } from '@/network/versions';
import { phoneName } from '@/device/name';
import { LockRow } from '@/lock/LockRow';
import { NotificationsRow } from '@/notify/NotificationsRow';
import { stopPush } from '@/notify/token';
import { Icon } from '@/ui/Icon';
import { Button } from '@/ui/controls';
import { Sheet } from '@/ui/Sheet';
import { restartToUpdate, useUpdateWaiting } from '@/updates/overTheAir';
import { fonts, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';

const VERSION = versionLabel(versionFromBuild(nativeBuildVersion) ?? nativeApplicationVersion, process.env.EXPO_PUBLIC_COMMIT ?? null, {
  id: Updates.updateId,
  createdAt: Updates.createdAt,
  embedded: Updates.isEmbeddedLaunch,
});

/** Who is signed in, this phone as the account knows it, and signing out. */
export function AccountSheet({ visible, onClose }: { visible: boolean; onClose: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const { signOut, getToken } = useAuth();
  const { user } = useUser();
  const id = useDeviceId();
  const status = useAccountStatus();
  const updateWaiting = useUpdateWaiting();
  const [leaving, setLeaving] = useState(false);
  const [unconfirmed, setUnconfirmed] = useState<string>();
  const provider = providerName(user?.externalAccounts[0]?.provider);
  const name = user?.fullName?.trim() || undefined;
  const email = user?.primaryEmailAddress?.emailAddress;
  const how = `Signed in with ${provider ?? 'email'}`;

  /** Takes the notification token and then the phone off the account first, so hosts hear of it; when that fails, it asks before leaving them there. */
  const leave = async (anyway = false) => {
    setLeaving(true);
    if (!anyway) {
      setUnconfirmed(undefined);
      try {
        await stopPush(() => getToken());
        await removePhone(() => getToken());
      } catch (error) {
        setLeaving(false);
        setUnconfirmed(whyNotConfirmed(error));
        return;
      }
    }
    try {
      await signOutHere(() => signOut(), { confirmed: !anyway });
      onClose();
    } finally {
      setLeaving(false);
    }
  };

  const close = () => {
    setUnconfirmed(undefined);
    onClose();
  };

  return (
    <Sheet visible={visible} onClose={close}>
      <View style={styles.head}>
        <Avatar size={44} />
        <View style={{ flex: 1 }}>
          <Text style={styles.name} numberOfLines={1}>
            {name ?? email ?? 'Signed in'}
          </Text>
          {name && email ? (
            <Text style={type.meta} numberOfLines={1}>
              {email}
            </Text>
          ) : null}
          <Text style={type.meta}>{how}</Text>
        </View>
      </View>
      <View style={styles.phone}>
        <Icon name="IconPhone" size={18} color={colors.secondary} />
        <View style={{ flex: 1 }}>
          <Text style={styles.phoneName}>{phoneName()}</Text>
          <Text style={type.meta}>
            This phone{id ? ' · ' : ''}
            {id ? <Text style={type.mono}>{shortKey(id)}</Text> : null}
          </Text>
          {status?.step === 'failed' ? <Text style={styles.problem}>Not on your account yet: {status.problem}</Text> : null}
        </View>
        {status?.step === 'failed' ? (
          <Pressable onPress={retryRegistration} accessibilityRole="button" hitSlop={8}>
            <Text style={styles.action}>Try again</Text>
          </Pressable>
        ) : null}
      </View>
      <NotificationsRow />
      <LockRow />
      {unconfirmed ? (
        <>
          <Text style={styles.noteTitle}>{unconfirmed}</Text>
          <Text style={[styles.note, styles.noteAfterTitle]}>This phone stays on your account until removed at app.sikemux.com.</Text>
          <View style={styles.choices}>
            <Button title="Try again" disabled={leaving} onPress={() => void leave()} />
            <Button
              kind="danger"
              icon="IconLogOut"
              title={leaving ? 'Signing out…' : 'Sign out anyway'}
              disabled={leaving}
              onPress={() => void leave(true)}
            />
          </View>
        </>
      ) : (
        <Button
          kind="danger"
          icon="IconLogOut"
          title={leaving ? 'Signing out…' : 'Sign out'}
          disabled={leaving}
          onPress={() => void leave()}
          style={styles.signOut}
        />
      )}
      <Pressable
        onPress={() => {
          close();
          router.push('/delete-account');
        }}
        disabled={leaving}
        style={styles.link}
        accessibilityRole="button">
        <Text style={styles.linkText}>Delete account…</Text>
      </Pressable>
      {updateWaiting ? (
        <Pressable onPress={restartToUpdate} style={styles.link} accessibilityRole="button">
          <Text style={[styles.linkText, styles.update]}>Restart to update</Text>
        </Pressable>
      ) : null}
      <Text style={styles.version}>Sikemux {VERSION}</Text>
    </Sheet>
  );
}

function whyNotConfirmed(error: unknown): string {
  return error instanceof ApiProblem && !error.unreachable ? error.message : "Can't reach Sikemux";
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    head: { flexDirection: 'row', alignItems: 'center', gap: 12, paddingHorizontal: 8, paddingTop: 6, paddingBottom: 14 },
    name: { fontFamily: fonts.uiSemibold, fontSize: 16, letterSpacing: -0.25, color: colors.ink },
    phone: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      minHeight: 52,
      paddingHorizontal: 10,
      paddingVertical: 6,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    phoneName: { ...type.row, fontSize: 15, color: colors.ink },
    note: { ...type.meta, lineHeight: 19, paddingHorizontal: 8, paddingTop: 10, paddingBottom: 12 },
    noteTitle: { fontFamily: fonts.uiSemibold, fontSize: 15, color: colors.ink, paddingHorizontal: 8, paddingTop: 12 },
    noteAfterTitle: { paddingTop: 4 },
    problem: { ...type.meta, color: colors.danger },
    action: { fontFamily: fonts.uiMedium, fontSize: 15, color: colors.accent },
    update: { color: colors.accent },
    choices: { gap: 8 },
    signOut: { marginTop: 16 },
    link: { height: 44, alignItems: 'center', justifyContent: 'center', marginTop: 4 },
    linkText: { fontFamily: fonts.uiMedium, fontSize: 15, color: colors.secondary },
    version: { ...type.meta, fontSize: 12, textAlign: 'center', paddingTop: 14 },
  });
};
