import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { useSignIn, useSignUp } from '@clerk/expo';
import { errorCode, explain } from '@protocol/account';

import { CodeEntry } from '@/ui/CodeEntry';
import { goBack, goHome } from '@/ui/navigate';
import { Button, Field, PasswordField } from '@/ui/controls';
import { Prompt } from '@/ui/prompt';
import { Nav, Screen } from '@/ui/screen';
import { fonts, type Palette, typeFor, useStyles } from '@/ui/theme';

/** Why an emailed code is asked for: a phone new to the account, a new account, or a new password. */
type CodePurpose = 'trust' | 'verify' | 'reset';

type Step =
  { kind: 'email' } | { kind: 'password' } | { kind: 'create' } | { kind: 'code'; purpose: CodePurpose } | { kind: 'new-password' };

const TITLES: Record<Step['kind'], string> = {
  email: 'Continue with email',
  password: 'Enter your password',
  create: 'Choose a password',
  code: 'Check your email',
  'new-password': 'Choose a new password',
};

export { Crashed as ErrorBoundary } from '@/screens/Crashed';

export default function SignIn() {
  const styles = useStyles(makeStyles);
  const { signIn } = useSignIn();
  const { signUp } = useSignUp();
  const [step, setStep] = useState<Step>({ kind: 'email' });
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();

  const go = (next: Step) => {
    setProblem(undefined);
    setPassword('');
    setCode('');
    setStep(next);
  };

  /** Runs one Clerk call, showing its problem instead of moving on when it has one. */
  const attempt = async (work: () => Promise<void>) => {
    setBusy(true);
    setProblem(undefined);
    try {
      await work();
    } catch (error) {
      setProblem(explain(error));
    } finally {
      setBusy(false);
    }
  };
  const check = (result: { error: unknown }) => {
    if (result.error) throw result.error;
  };

  const finishSignIn = async () => {
    check(await signIn.finalize());
    goHome();
  };

  /** After a password, a phone new to the account confirms with an emailed code. */
  const afterPassword = async () => {
    if (signIn.status === 'complete') return finishSignIn();
    if (signIn.status === 'needs_client_trust' || signIn.status === 'needs_second_factor') {
      check(await signIn.mfa.sendEmailCode());
      return go({ kind: 'code', purpose: 'trust' });
    }
    throw new Error('This account needs a sign-in step the app does not do yet; sign in on the web first.');
  };

  const submitEmail = () =>
    attempt(async () => {
      const { error } = await signIn.create({ identifier: email.trim() });
      if (errorCode(error) === 'form_identifier_not_found') return go({ kind: 'create' });
      check({ error });
      go({ kind: 'password' });
    });

  const submitPassword = () =>
    attempt(async () => {
      check(await signIn.password({ password }));
      await afterPassword();
    });

  const submitNewAccount = () =>
    attempt(async () => {
      check(await signUp.password({ emailAddress: email.trim(), password }));
      check(await signUp.verifications.sendEmailCode());
      go({ kind: 'code', purpose: 'verify' });
    });

  const forgot = () =>
    attempt(async () => {
      check(await signIn.resetPasswordEmailCode.sendCode());
      go({ kind: 'code', purpose: 'reset' });
    });

  const submitNewPassword = () =>
    attempt(async () => {
      check(await signIn.resetPasswordEmailCode.submitPassword({ password }));
      await afterPassword();
    });

  const submitCode = (purpose: CodePurpose, typed: string) =>
    attempt(async () => {
      if (purpose === 'trust') {
        check(await signIn.mfa.verifyEmailCode({ code: typed }));
        return finishSignIn();
      }
      if (purpose === 'reset') {
        check(await signIn.resetPasswordEmailCode.verifyCode({ code: typed }));
        return go({ kind: 'new-password' });
      }
      check(await signUp.verifications.verifyEmailCode({ code: typed }));
      if (signUp.status !== 'complete') throw new Error('The account needs more than this app asks for; finish signing up on the web.');
      check(await signUp.finalize());
      goHome();
    }).finally(() => setCode(''));

  const typeCode = (typed: string) => {
    setCode(typed);
    if (typed.length === 6 && step.kind === 'code') void submitCode(step.purpose, typed);
  };

  const back = () => {
    if (step.kind === 'email') return goBack();
    void signIn.reset();
    go({ kind: 'email' });
  };

  const detail =
    step.kind === 'email'
      ? undefined
      : step.kind === 'code'
        ? `Sikemux sent a code to ${email}.`
        : step.kind === 'create'
          ? 'At least 15 characters.'
          : email;

  return (
    <Screen>
      <Nav back="Back" onBack={back} />
      <Prompt icon="Logo" title={TITLES[step.kind]} detail={detail}>
        {step.kind === 'code' ? (
          <>
            <View style={styles.tiles}>
              <CodeEntry code={code} onChange={typeCode} disabled={busy} failed={problem !== undefined && code.length === 0} />
            </View>
            {problem ? <Text style={[styles.problem, styles.centered]}>{problem}</Text> : null}
          </>
        ) : null}
      </Prompt>
      {step.kind === 'code' ? null : (
        <View style={styles.form}>
          {step.kind === 'email' ? (
            <Field
              key="email"
              icon="IconInbox"
              value={email}
              onChangeText={setEmail}
              placeholder="you@example.com"
              autoFocus
              autoCapitalize="none"
              autoCorrect={false}
              keyboardType="email-address"
              textContentType="emailAddress"
              autoComplete="email"
              returnKeyType="next"
              onSubmitEditing={() => void submitEmail()}
            />
          ) : (
            <PasswordField
              key={step.kind}
              value={password}
              onChangeText={setPassword}
              placeholder="Password"
              autoFocus
              textContentType={step.kind === 'password' ? 'password' : 'newPassword'}
              autoComplete={step.kind === 'password' ? 'current-password' : 'new-password'}
              returnKeyType="go"
              onSubmitEditing={() =>
                void (step.kind === 'password' ? submitPassword() : step.kind === 'create' ? submitNewAccount() : submitNewPassword())
              }
            />
          )}
          {problem ? <Text style={styles.problem}>{problem}</Text> : null}
          <Button
            kind="primary"
            title={
              step.kind === 'email'
                ? 'Continue'
                : step.kind === 'password'
                  ? 'Sign in'
                  : step.kind === 'create'
                    ? 'Create account'
                    : 'Save password'
            }
            disabled={busy || (step.kind === 'email' ? !email.trim() : !password)}
            onPress={() =>
              void (step.kind === 'email'
                ? submitEmail()
                : step.kind === 'password'
                  ? submitPassword()
                  : step.kind === 'create'
                    ? submitNewAccount()
                    : submitNewPassword())
            }
          />
          {step.kind === 'password' ? (
            <Pressable onPress={() => void forgot()} disabled={busy} style={styles.link} accessibilityRole="button">
              <Text style={styles.linkText}>Forgot your password?</Text>
            </Pressable>
          ) : null}
        </View>
      )}
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    form: { paddingTop: 24, paddingHorizontal: 24, gap: 12 },
    centered: { textAlign: 'center' },
    tiles: { marginTop: 24 },
    problem: { ...type.meta, color: colors.danger, marginTop: 4 },
    link: { minHeight: 44, alignItems: 'center', justifyContent: 'center' },
    linkText: { fontFamily: fonts.uiMedium, fontSize: 15, color: colors.secondary },
  });
};
