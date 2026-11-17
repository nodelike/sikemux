import { useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { useSignIn, useSignUp } from '@clerk/expo';
import { explain, signInFlow, type CodePurpose, type SignInStep } from '@shared/account/account';

import { type Translate, useT } from '@/i18n';
import { CodeEntry } from '@/ui/CodeEntry';
import { goBack, goHome } from '@/ui/navigate';
import { Button, Field, PasswordField } from '@/ui/controls';
import { Prompt } from '@/ui/prompt';
import { Nav, Screen } from '@/ui/screen';
import { fonts, type Palette, typeFor, useStyles } from '@/ui/theme';

function titleOf(kind: SignInStep['kind'], t: Translate): string {
  switch (kind) {
    case 'email':
      return t('Continue with email');
    case 'password':
      return t('Enter your password');
    case 'create':
      return t('Choose a password');
    case 'code':
      return t('Check your email');
    case 'new-password':
      return t('Choose a new password');
  }
}

export { Crashed as ErrorBoundary } from '@/ui/Crashed';

export default function SignIn() {
  const styles = useStyles(makeStyles);
  const t = useT();
  const { signIn } = useSignIn();
  const { signUp } = useSignUp();
  const [step, setStep] = useState<SignInStep>({ kind: 'email' });
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();

  const go = (next: SignInStep) => {
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
  const flow = signInFlow({
    signIn,
    signUp,
    go,
    done: goHome,
    unsupported: {
      signIn: t('This account needs a sign-in step the app does not do yet; sign in on the web first.'),
      signUp: t('The account needs more than this app asks for; finish signing up on the web.'),
    },
  });

  const submitEmail = () => attempt(() => flow.submitEmail(email));
  const submitPassword = () => attempt(() => flow.submitPassword(password));
  const submitNewAccount = () => attempt(() => flow.submitNewAccount(email, password));
  const forgot = () => attempt(flow.forgot);
  const submitNewPassword = () => attempt(() => flow.submitNewPassword(password));
  const submitCode = (purpose: CodePurpose, typed: string) => attempt(() => flow.submitCode(purpose, typed)).finally(() => setCode(''));

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
        ? t('Sikemux sent a code to {email}.', { email })
        : step.kind === 'create'
          ? t('At least 15 characters.')
          : email;

  return (
    <Screen>
      <Nav back={t('Back')} onBack={back} />
      <Prompt icon="Logo" title={titleOf(step.kind, t)} detail={detail}>
        {step.kind === 'code' ? (
          <>
            <View style={styles.tiles}>
              <CodeEntry code={code} onChange={typeCode} disabled={busy} failed={problem !== undefined && code.length === 0} />
            </View>
            {problem ? <Text style={[styles.problem, styles.centered]}>{t(problem)}</Text> : null}
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
              placeholder={t('Password')}
              autoFocus
              textContentType={step.kind === 'password' ? 'password' : 'newPassword'}
              autoComplete={step.kind === 'password' ? 'current-password' : 'new-password'}
              returnKeyType="go"
              onSubmitEditing={() =>
                void (step.kind === 'password' ? submitPassword() : step.kind === 'create' ? submitNewAccount() : submitNewPassword())
              }
            />
          )}
          {problem ? <Text style={styles.problem}>{t(problem)}</Text> : null}
          <Button
            kind="primary"
            title={
              step.kind === 'email'
                ? t('Continue')
                : step.kind === 'password'
                  ? t('Sign in')
                  : step.kind === 'create'
                    ? t('Create account')
                    : t('Save password')
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
              <Text style={styles.linkText}>{t('Forgot your password?')}</Text>
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
