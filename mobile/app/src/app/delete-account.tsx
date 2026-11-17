import { useReducer } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { useAuth, useUser } from '@clerk/expo';
import { CONFIRM_WORD, explain } from '@shared/account/account';

import { deleteAccount } from '@/account/api';
import { ConfirmItsYou, ReverifyCancelled, useRecentSignIn } from '@/account/ConfirmItsYou';
import { confirmed, deletion, START } from '@/account/deletion';
import { signOutHere } from '@/account/leave';
import { RETENTION } from '@/account/config';
import { useT } from '@/i18n';
import { Button, Field } from '@/ui/controls';
import { Prompt } from '@/ui/prompt';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

/** Deletes the account, after the person types the word and, when the server asks, proves it is them again. */
export default function DeleteAccount() {
  const styles = useStyles(makeStyles);
  const t = useT();
  const bottom = useBottomGap();
  const { getToken, signOut } = useAuth();
  const { user } = useUser();
  const email = user?.primaryEmailAddress?.emailAddress;
  const [state, dispatch] = useReducer(deletion, START);
  const recent = useRecentSignIn(t('Delete it at app.sikemux.com/delete-account.'));

  const start = async () => {
    if (!confirmed(state) || state.phase.name !== 'confirm') return;
    dispatch({ type: 'delete' });
    try {
      await recent.run(() => deleteAccount(() => getToken({ skipCache: true })));
    } catch (error) {
      if (error instanceof ReverifyCancelled) dispatch({ type: 'cancelled' });
      else dispatch({ type: 'failed', problem: explain(error) });
      return;
    }
    dispatch({ type: 'deleted' });
    await signOutHere(() => signOut(), { farewell: 'deleted-here', confirmed: true });
  };

  if (recent.verify)
    return (
      <ConfirmItsYou
        verify={recent.verify}
        onEnter={recent.enter}
        onCheck={(entry) => void recent.check(entry)}
        onBack={recent.cancel}
        action={{ title: t('Delete account'), icon: 'IconTrash', kind: 'danger' }}
      />
    );

  const phase = state.phase;
  const busy = phase.name !== 'confirm';
  const gone = [
    t('Hosts are signed out.'),
    t('Phones, this one too, are signed out and removed from every host.'),
    t('After {days} days your sign-in, device keys and history are erased, and backups {backupDays} days later.', {
      days: RETENTION.restorableDays,
      backupDays: RETENTION.backupDays,
    }),
  ];
  const [deletesBefore, deletesAfter = ''] = t('This deletes {account}. You can restore it within 30 days.').split('{account}');
  const [typeBefore, typeAfter = ''] = t('Type {word} to confirm').split('{word}');
  return (
    <Screen>
      <Nav back={t('Back')} onBack={() => router.back()} />
      <ScrollView
        contentContainerStyle={{ paddingBottom: bottom }}
        keyboardShouldPersistTaps="handled"
        showsVerticalScrollIndicator={false}>
        <Prompt
          icon="IconTrash"
          tone="danger"
          title={t('Delete your account')}
          detail={
            email ? (
              <>
                {deletesBefore}
                <Text style={styles.ink}>{email}</Text>
                {deletesAfter}
              </>
            ) : (
              t('This deletes your account. You can restore it within 30 days.')
            )
          }
        />
        <View style={styles.form}>
          <View style={styles.list}>
            {gone.map((line) => (
              <View key={line} style={styles.item}>
                <Text style={styles.bullet}>•</Text>
                <Text style={styles.itemText}>{line}</Text>
              </View>
            ))}
          </View>
          <Text style={styles.label}>
            {typeBefore}
            <Text style={styles.word}>{CONFIRM_WORD}</Text>
            {typeAfter}
          </Text>
          <Field
            value={state.typed}
            onChangeText={(text) => dispatch({ type: 'typed', text })}
            placeholder={CONFIRM_WORD}
            editable={!busy}
            autoCapitalize="none"
            autoCorrect={false}
            returnKeyType="go"
            onSubmitEditing={() => void start()}
          />
          {state.problem ? <Text style={styles.problem}>{t(state.problem)}</Text> : null}
          <Button
            kind="danger"
            icon="IconTrash"
            title={busy ? t('Deleting…') : t('Delete account')}
            disabled={busy || !confirmed(state)}
            onPress={() => void start()}
          />
        </View>
      </ScrollView>
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    form: { paddingTop: 24, paddingHorizontal: 24, gap: 12 },
    list: { gap: 8 },
    item: { flexDirection: 'row', gap: 10, paddingRight: 8 },
    bullet: { ...type.body, color: colors.tertiary },
    itemText: { ...type.body, flex: 1 },
    label: { ...type.meta, marginTop: 8, marginBottom: -4 },
    word: { ...type.mono, color: colors.ink },
    ink: { color: colors.ink },
    problem: { ...type.meta, color: colors.danger },
  });
};
