import { useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { useAuth } from '@clerk/expo';

import { AccountProblem, restoreAccount } from '@/account/api';
import { signOutHere } from '@/account/leave';
import { retryRegistration } from '@/account/session';
import { Button } from '@/ui/controls';
import { Screen, useBottomGap } from '@/ui/screen';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

/** Signed in to an account that was deleted but can still be restored. */
export function RestoreAccount({ deletesAt }: { deletesAt: string }) {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  const { getToken, signOut } = useAuth();
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const day = new Date(deletesAt).toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric' });

  const restore = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      await restoreAccount(() => getToken({ skipCache: true }));
      retryRegistration();
    } catch (error) {
      setBusy(false);
      setProblem(error instanceof AccountProblem ? error.message : 'Something went wrong; try again.');
    }
  };

  return (
    <Screen>
      <View style={styles.block}>
        <Text style={styles.title}>Your account is being deleted</Text>
        <Text style={styles.body}>
          It will be deleted for good on <Text style={styles.ink}>{day}</Text>. Restore it to keep using Sikemux. Your devices were signed
          out, so each one signs in again.
        </Text>
        {problem ? <Text style={styles.problem}>{problem}</Text> : null}
      </View>
      <View style={[styles.footer, { paddingBottom: bottom }]}>
        <Button kind="primary" title="Restore account" busy={busy} disabled={busy} onPress={() => void restore()} />
        <Button title="Sign out" disabled={busy} onPress={() => void signOutHere(() => signOut(), { confirmed: true })} />
      </View>
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    block: { flex: 1, justifyContent: 'center', paddingHorizontal: 32, paddingBottom: 120 },
    title: { ...type.title, fontSize: 22, textAlign: 'center' },
    body: { ...type.body, textAlign: 'center', marginTop: 10 },
    ink: { color: colors.ink },
    problem: { ...type.meta, color: colors.danger, textAlign: 'center', marginTop: 12 },
    footer: { paddingHorizontal: 16, paddingTop: 12, gap: 10 },
  });
};
