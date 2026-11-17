import { formatDay } from '@shared/lib/format';
import { useState } from 'react';
import { StyleSheet, Text } from 'react-native';
import { useAuth } from '@clerk/expo';

import { api } from '@/account/api';
import { problem as problemOf } from '@/core/problem';
import { signOutHere } from '@/account/leave';
import { useT } from '@/i18n';
import { retryRegistration } from '@/account/session';
import { Button } from '@/ui/controls';
import { PromptScreen } from '@/ui/prompt';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

/** Signed in to an account that was deleted but can still be restored. */
export function RestoreAccount({ deletesAt }: { deletesAt: string }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  const { getToken, signOut } = useAuth();
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const day = formatDay(deletesAt);
  const [before, after = ''] = t('It will be deleted for good on {day}.').split('{day}');

  const restore = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      await api(() => getToken({ skipCache: true })).restoreAccount();
      retryRegistration();
    } catch (error) {
      setBusy(false);
      setProblem(problemOf(error));
    }
  };

  return (
    <PromptScreen
      icon="IconHourglass"
      title={t('Your account is being deleted')}
      detail={
        <>
          {before}
          <Text style={styles.ink}>{day}</Text>
          {after}
        </>
      }
      actions={
        <>
          <Button kind="primary" title={t('Restore account')} busy={busy} disabled={busy} onPress={() => void restore()} />
          <Button
            title={t('Sign out')}
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
