import { useReducer, useRef } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { useAuth, useReverification, useSession, useUser } from '@clerk/expo';

import { AccountProblem, deleteAccount, ReverifyNeeded } from '@/account/api';
import { appleConsent } from '@/account/apple';
import { errorCode, explain } from '@/account/clerkErrors';
import { chooseFactor, CONFIRM_WORD, confirmed, deletion, REVERIFY_HINT, START } from '@/account/deletion';
import { signOutHere } from '@/account/leave';
import { CodeEntry } from '@/ui/CodeEntry';
import { Button, Field, PasswordField } from '@/ui/controls';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

const GONE = [
  'Hosts are signed out. The phones paired to them keep working.',
  'Phones, this one too, are signed out and removed from every host.',
  'After 30 days your sign-in, device keys and the account’s history are erased, and backups within 30 days after that.',
];

function describe(error: unknown): string {
  return error instanceof AccountProblem ? error.message : explain(error);
}

/** Deletes the account, after the person types the word and, when the server asks, proves it is them again. */
export default function DeleteAccount() {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  const { getToken, signOut } = useAuth();
  const { session } = useSession();
  const { user } = useUser();
  const email = user?.primaryEmailAddress?.emailAddress;
  const [state, dispatch] = useReducer(deletion, START);
  const pending = useRef<{ complete: () => void; cancel: () => void }>(undefined);
  const appleCode = useRef<string>(undefined);

  const askAgain = async (cancel: () => void) => {
    try {
      if (!session) throw new Error('Sign in again to delete your account.');
      const verification = await session.startVerification({ level: 'first_factor' });
      const chosen = chooseFactor(verification.supportedFirstFactors ?? []);
      if (!chosen) throw new Error("This account can't confirm it's you in the app. Delete it at app.sikemux.com/delete-account.");
      if (chosen.emailAddressId) {
        await session.prepareFirstFactorVerification({ strategy: 'email_code', emailAddressId: chosen.emailAddressId });
      }
      dispatch({ type: 'reverify', factor: chosen.factor });
    } catch (error) {
      pending.current = undefined;
      cancel();
      dispatch({ type: 'failed', problem: describe(error) });
    }
  };

  const remove = useReverification(
    async () => {
      try {
        return await deleteAccount(() => getToken({ skipCache: true }), appleCode.current);
      } catch (error) {
        if (error instanceof ReverifyNeeded) return REVERIFY_HINT;
        throw error;
      }
    },
    {
      onNeedsReverification: ({ complete, cancel }) => {
        pending.current = { complete, cancel };
        void askAgain(cancel);
      },
    },
  );

  const start = async () => {
    if (!confirmed(state) || state.phase.name !== 'confirm') return;
    dispatch({ type: 'delete' });
    const apple = await appleConsent(user?.externalAccounts);
    if (apple.cancelled) {
      dispatch({ type: 'cancelled' });
      return;
    }
    appleCode.current = apple.code;
    try {
      await remove();
    } catch (error) {
      if (errorCode(error) === 'reverification_cancelled') dispatch({ type: 'cancelled' });
      else dispatch({ type: 'failed', problem: describe(error) });
      return;
    }
    dispatch({ type: 'deleted' });
    await signOutHere(() => signOut(), { farewell: 'deleted-here', confirmed: true });
  };

  const check = async (entry: string) => {
    if (state.phase.name !== 'verify' || !session) return;
    const factor = state.phase.factor;
    dispatch({ type: 'checking' });
    try {
      const result = await session.attemptFirstFactorVerification(
        factor.strategy === 'password' ? { strategy: 'password', password: entry } : { strategy: 'email_code', code: entry },
      );
      if (result.status !== 'complete') {
        pending.current?.cancel();
        pending.current = undefined;
        dispatch({
          type: 'failed',
          problem: 'This account also needs a second step to confirm it. Delete it at app.sikemux.com/delete-account.',
        });
        return;
      }
    } catch (error) {
      dispatch({ type: 'rejected', problem: explain(error) });
      return;
    }
    dispatch({ type: 'verified' });
    pending.current?.complete();
    pending.current = undefined;
  };

  const back = () => {
    if (state.phase.name === 'verify') {
      pending.current?.cancel();
      pending.current = undefined;
      return;
    }
    router.back();
  };

  const phase = state.phase;
  if (phase.name === 'verify') {
    const byEmail = phase.factor.strategy === 'email_code';
    return (
      <Screen>
        <Nav back="Back" onBack={back} />
        <View style={byEmail ? styles.codeBlock : styles.block}>
          <Text style={[styles.title, byEmail && styles.centered]}>Confirm it&apos;s you</Text>
          <Text style={[styles.detail, byEmail && styles.centered]}>
            {phase.factor.strategy === 'email_code'
              ? `Sikemux sent a code to ${phase.factor.to}. Enter it to delete your account.`
              : 'Enter your password to delete your account.'}
          </Text>
          {byEmail ? (
            <View style={styles.tiles}>
              <CodeEntry
                code={phase.entry}
                disabled={phase.checking}
                failed={state.problem !== undefined && phase.entry.length === 0}
                onChange={(code) => {
                  dispatch({ type: 'entered', entry: code });
                  if (code.length === 6) void check(code);
                }}
              />
            </View>
          ) : (
            <>
              <PasswordField
                value={phase.entry}
                onChangeText={(entry) => dispatch({ type: 'entered', entry })}
                placeholder="Password"
                autoFocus
                textContentType="password"
                autoComplete="current-password"
                returnKeyType="go"
                onSubmitEditing={() => void check(phase.entry)}
              />
              <Button
                kind="danger"
                title={phase.checking ? 'Checking…' : 'Delete account'}
                disabled={phase.checking || !phase.entry}
                onPress={() => void check(phase.entry)}
              />
            </>
          )}
          {state.problem ? <Text style={[styles.problem, byEmail && styles.centered]}>{state.problem}</Text> : null}
        </View>
      </Screen>
    );
  }

  const busy = phase.name !== 'confirm';
  return (
    <Screen>
      <Nav back="Back" onBack={back} />
      <ScrollView contentContainerStyle={[styles.block, { paddingBottom: bottom }]} keyboardShouldPersistTaps="handled">
        <Text style={styles.title}>Delete your account</Text>
        <Text style={styles.detail}>
          This deletes {email ? <Text style={styles.ink}>{email}</Text> : 'it'} from Sikemux. Changed your mind? Sign in again within 30
          days to restore it.
        </Text>
        <View style={styles.list}>
          {GONE.map((line) => (
            <View key={line} style={styles.item}>
              <Text style={styles.bullet}>•</Text>
              <Text style={styles.itemText}>{line}</Text>
            </View>
          ))}
        </View>
        <Text style={styles.detail}>Your code and files never reached us, so they stay on your hosts.</Text>
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
          title={busy ? 'Deleting…' : 'Delete account'}
          disabled={busy || !confirmed(state)}
          onPress={() => void start()}
        />
      </ScrollView>
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    block: { paddingTop: 40, paddingHorizontal: 24, gap: 12 },
    codeBlock: { paddingTop: 40, paddingHorizontal: 28, alignItems: 'center', gap: 12 },
    title: { ...type.title, fontSize: 22 },
    detail: { ...type.body },
    centered: { textAlign: 'center' },
    list: { gap: 8 },
    item: { flexDirection: 'row', gap: 10, paddingRight: 8 },
    bullet: { ...type.body, color: colors.tertiary },
    itemText: { ...type.body, flex: 1 },
    label: { ...type.meta, marginTop: 8, marginBottom: -4 },
    word: { ...type.mono, color: colors.ink },
    ink: { color: colors.ink },
    tiles: { marginTop: 12 },
    problem: { ...type.meta, color: colors.danger },
  });
};
