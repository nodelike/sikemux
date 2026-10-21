import { useReducer, useRef } from 'react';
import { StyleSheet, Text, View } from 'react-native';
import { useReverification, useSession } from '@clerk/expo';

import { chooseFactor, errorCode, explain, orReverify } from '@protocol/account';

import { type Verify, verifying } from '@/account/reverify';
import { CodeEntry } from '@/ui/CodeEntry';
import { Button, PasswordField } from '@/ui/controls';
import type { IconName } from '@/ui/icons.generated';
import { Prompt } from '@/ui/prompt';
import { Nav, Screen } from '@/ui/screen';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

/** The person backed out of proving it is them; nothing was done. */
export class ReverifyCancelled extends Error {}

type Pending = { complete: () => void; cancel: () => void };

/**
 * Runs requests the server takes only after a sign-in from the last ten minutes. When it asks for one,
 * `verify` holds what the person must prove, and the request carries on by itself once they have.
 * `elsewhere` says where else to do it, for an account the app can't verify.
 */
export function useRecentSignIn(elsewhere: string) {
  const { session } = useSession();
  const [verify, dispatch] = useReducer(verifying, undefined);
  const pending = useRef<Pending>(undefined);
  const request = useRef<() => Promise<unknown>>(undefined);
  const failure = useRef<string>(undefined);

  const giveUp = (problem?: string) => {
    failure.current = problem;
    pending.current?.cancel();
    pending.current = undefined;
  };

  const ask = async () => {
    try {
      if (!session) throw new Error('Sign in again.');
      const started = await session.startVerification({ level: 'first_factor' });
      const factor = chooseFactor(started.supportedFirstFactors ?? []);
      if (!factor) throw new Error(`This account can't confirm it's you in the app. ${elsewhere}`);
      if (factor.strategy === 'email_code') {
        await session.prepareFirstFactorVerification({ strategy: 'email_code', emailAddressId: factor.emailAddressId });
      }
      dispatch({ type: 'ask', factor });
    } catch (error) {
      giveUp(explain(error));
    }
  };

  const reverified = useReverification(() => orReverify(async () => request.current?.()), {
    onNeedsReverification: (next) => {
      pending.current = next;
      void ask();
    },
  });

  /** Runs `work`, asking the person to prove it is them when the server wants it. Throws ReverifyCancelled when they back out. */
  const run = async <T,>(work: () => Promise<T>): Promise<T> => {
    request.current = work;
    failure.current = undefined;
    try {
      return (await reverified()) as T;
    } catch (error) {
      if (errorCode(error) !== 'reverification_cancelled') throw error;
      if (failure.current) throw new Error(failure.current);
      throw new ReverifyCancelled();
    } finally {
      dispatch({ type: 'done' });
    }
  };

  const check = async (entry: string) => {
    if (!verify || !session) return;
    dispatch({ type: 'checking' });
    try {
      const result = await session.attemptFirstFactorVerification(
        verify.factor.strategy === 'password' ? { strategy: 'password', password: entry } : { strategy: 'email_code', code: entry },
      );
      if (result.status !== 'complete') {
        giveUp(`This account also needs a second step to confirm it. ${elsewhere}`);
        return;
      }
    } catch (error) {
      dispatch({ type: 'rejected', problem: explain(error) });
      return;
    }
    pending.current?.complete();
    pending.current = undefined;
  };

  return {
    run,
    verify,
    enter: (entry: string) => dispatch({ type: 'entered', entry }),
    check,
    cancel: () => giveUp(),
  };
}

/** Asks for the password, or the code sent to the account's email, whose sixth digit sends it. */
export function ConfirmItsYou({
  verify,
  onEnter,
  onCheck,
  onBack,
  action,
}: {
  verify: Verify;
  onEnter: (entry: string) => void;
  onCheck: (entry: string) => void;
  onBack: () => void;
  /** The button under the password, named for what it goes on to do. */
  action: { title: string; icon?: IconName; kind: 'primary' | 'danger' };
}) {
  const styles = useStyles(makeStyles);
  const { factor, entry, checking, problem } = verify;
  const byEmail = factor.strategy === 'email_code';
  return (
    <Screen>
      <Nav back="Back" onBack={onBack} />
      <Prompt icon="IconShield" title="Confirm it's you" detail={byEmail ? `Sikemux sent a code to ${factor.to}.` : undefined}>
        {byEmail ? (
          <View style={styles.tiles}>
            <CodeEntry
              code={entry}
              disabled={checking}
              failed={problem !== undefined && entry.length === 0}
              onChange={(code) => {
                onEnter(code);
                if (code.length === 6) onCheck(code);
              }}
            />
          </View>
        ) : null}
        {byEmail && problem ? <Text style={[styles.problem, styles.underTiles]}>{problem}</Text> : null}
      </Prompt>
      {byEmail ? null : (
        <View style={styles.form}>
          <PasswordField
            value={entry}
            onChangeText={onEnter}
            placeholder="Password"
            autoFocus
            textContentType="password"
            autoComplete="current-password"
            returnKeyType="go"
            onSubmitEditing={() => onCheck(entry)}
          />
          <Button
            kind={action.kind}
            icon={action.icon}
            title={checking ? 'Checking…' : action.title}
            disabled={checking || !entry}
            onPress={() => onCheck(entry)}
          />
          {problem ? <Text style={styles.problem}>{problem}</Text> : null}
        </View>
      )}
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    form: { paddingTop: 24, paddingHorizontal: 24, gap: 12 },
    tiles: { marginTop: 24 },
    underTiles: { textAlign: 'center', marginTop: 12 },
    problem: { ...type.meta, color: colors.danger },
  });
};
