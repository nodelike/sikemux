import { useReducer } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { useAuth, useUser } from '@clerk/expo';
import { CONFIRM_WORD, explain } from '@protocol/account';

import { deleteAccount } from '@/account/api';
import { appleConsent } from '@/account/apple';
import { ConfirmItsYou, ReverifyCancelled, useRecentSignIn } from '@/account/ConfirmItsYou';
import { confirmed, deletion, START } from '@/account/deletion';
import { signOutHere } from '@/account/leave';
import { Button, Field } from '@/ui/controls';
import { Prompt } from '@/ui/prompt';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

const GONE = [
  'Hosts are signed out.',
  'Phones, this one too, are signed out and removed from every host.',
  'After 30 days your sign-in, device keys and history are erased, and backups 30 days later.',
];

/** Deletes the account, after the person types the word and, when the server asks, proves it is them again. */
export default function DeleteAccount() {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  const { getToken, signOut } = useAuth();
  const { user } = useUser();
  const email = user?.primaryEmailAddress?.emailAddress;
  const [state, dispatch] = useReducer(deletion, START);
  const recent = useRecentSignIn('Delete it at app.sikemux.com/delete-account.');

  const start = async () => {
    if (!confirmed(state) || state.phase.name !== 'confirm') return;
    dispatch({ type: 'delete' });
    const apple = await appleConsent(user?.externalAccounts);
    if (apple.cancelled) {
      dispatch({ type: 'cancelled' });
      return;
    }
    try {
      await recent.run(() => deleteAccount(() => getToken({ skipCache: true }), apple.code));
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
        action={{ title: 'Delete account', icon: 'IconTrash', kind: 'danger' }}
      />
    );

  const phase = state.phase;
  const busy = phase.name !== 'confirm';
  return (
    <Screen>
      <Nav back="Back" onBack={() => router.back()} />
      <ScrollView
        contentContainerStyle={{ paddingBottom: bottom }}
        keyboardShouldPersistTaps="handled"
        showsVerticalScrollIndicator={false}>
        <Prompt
          icon="IconTrash"
          tone="danger"
          title="Delete your account"
          detail={<>This deletes {email ? <Text style={styles.ink}>{email}</Text> : 'your account'}. You can restore it within 30 days.</>}
        />
        <View style={styles.form}>
          <View style={styles.list}>
            {GONE.map((line) => (
              <View key={line} style={styles.item}>
                <Text style={styles.bullet}>•</Text>
                <Text style={styles.itemText}>{line}</Text>
              </View>
            ))}
          </View>
          <Text style={styles.label}>
            Type <Text style={styles.word}>{CONFIRM_WORD}</Text> to confirm
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
          {state.problem ? <Text style={styles.problem}>{state.problem}</Text> : null}
          <Button
            kind="danger"
            icon="IconTrash"
            title={busy ? 'Deleting…' : 'Delete account'}
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
