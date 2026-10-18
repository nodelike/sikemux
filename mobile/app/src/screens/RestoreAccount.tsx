import { useState } from 'react';
import { StyleSheet, Text } from 'react-native';
import { useAuth } from '@clerk/expo';

import { AccountProblem, restoreAccount } from '@/account/api';
import { signOutHere } from '@/account/leave';
import { retryRegistration } from '@/account/session';
import { Button } from '@/ui/controls';
import { PromptScreen } from '@/ui/prompt';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

/** Signed in to an account that was deleted but can still be restored. */
export function RestoreAccount({ deletesAt }: { deletesAt: string }) {
  const styles = useStyles(makeStyles);
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
    <PromptScreen
      icon="IconHourglass"
      title="Your account is being deleted"
      detail={
        <>
          It will be deleted for good on <Text style={styles.ink}>{day}</Text>.
        </>
      }
      actions={
        <>
          <Button kind="primary" title="Restore account" busy={busy} disabled={busy} onPress={() => void restore()} />
          <Button
            title="Sign out"
            icon="IconLogOut"
            disabled={busy}
            onPress={() => void signOutHere(() => signOut(), { confirmed: true })}
          />
        </>
      }>
      {problem ? <Text style={styles.problem}>{problem}</Text> : null}
    </PromptScreen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    ink: { color: colors.ink },
    problem: { ...type.meta, color: colors.danger, textAlign: 'center', marginTop: 12 },
  });
};
