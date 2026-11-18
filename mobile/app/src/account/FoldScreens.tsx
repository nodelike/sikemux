import { useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';
import { router } from 'expo-router';
import { useAuth } from '@clerk/expo';
import type { FoldCatch, FoldPreview } from '@sikemux/protocol';
import { errorCode } from '@shared/account/account';

import { api, ApiProblem } from '@/account/api';
import { ConfirmItsYou, ReverifyCancelled, useRecentSignIn } from '@/account/ConfirmItsYou';
import { checkFoldAgain, dropFold, type FoldGate, holdFold, openFold, useWaitingFold, type Waiting } from '@/account/fold';
import { signOutHere } from '@/account/leave';
import { RestoreAccount } from '@/account/RestoreAccount';
import { problem as problemOf } from '@/core/problem';
import { useT } from '@/i18n';
import { Button } from '@/ui/controls';
import { PromptScreen } from '@/ui/prompt';
import { Screen } from '@/ui/screen';
import { type Palette, typeFor, useColors, useStyles } from '@/ui/theme';

const ELSEWHERE = 'Bring your accounts together at app.sikemux.com.';

/** What the signed-in account sees until the gate opens: whether the person already has an account, or the fold to confirm. */
export function FoldScreen({ gate }: { gate: Exclude<FoldGate, { kind: 'open' }> }) {
  if (gate.kind === 'catch') return <Catch caught={gate.caught} />;
  if (gate.kind === 'confirm') return <ConfirmFold waiting={gate.waiting} />;
  return <Checking />;
}

function Checking() {
  const colors = useColors();
  return (
    <Screen style={{ justifyContent: 'center' }}>
      <ActivityIndicator color={colors.secondary} />
    </Screen>
  );
}

function Catch({ caught }: { caught: FoldCatch }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  const { getToken, signOut, userId } = useAuth();
  const recent = useRecentSignIn(t(ELSEWHERE));
  const [busy, setBusy] = useState<'start' | 'keep'>();
  const [problem, setProblem] = useState<string>();

  if (caught.deletesAt) return <RestoreAccount deletesAt={caught.deletesAt} onRestored={checkFoldAgain} />;
  if (recent.verify)
    return (
      <ConfirmItsYou
        verify={recent.verify}
        onEnter={recent.enter}
        onCheck={(entry) => void recent.check(entry)}
        onBack={recent.cancel}
        action={{ title: t('Use my account'), kind: 'primary' }}
      />
    );

  const why = caught.why ?? 'browser';
  const token = () => getToken({ skipCache: true });

  const startFold = async () => {
    setBusy('start');
    setProblem(undefined);
    try {
      const intent = await recent.run(() => api(token).startFold({ why }));
      holdFold({ id: intent.id, secret: intent.secret, expiresAt: intent.expiresAt, why, waitingWords: intent.waitingWords });
      await signOutHere(() => signOut(), { confirmed: true });
    } catch (error) {
      setBusy(undefined);
      if (!(error instanceof ReverifyCancelled)) setProblem(problemOf(error));
    }
  };

  const keepNew = async () => {
    setBusy('keep');
    setProblem(undefined);
    try {
      await api(token).keepNewAccount();
      if (userId) await openFold(userId);
    } catch (error) {
      setBusy(undefined);
      setProblem(problemOf(error));
    }
  };

  return (
    <PromptScreen
      icon={caught.mustFold ? 'IconShield' : 'IconUser'}
      title={caught.title ?? ''}
      detail={caught.address ? <Text style={styles.ink}>{caught.address}</Text> : undefined}
      actions={
        <>
          <Button kind="primary" title={t('Use my account')} busy={busy === 'start'} disabled={!!busy} onPress={() => void startFold()} />
          {caught.mustFold ? (
            <Button
              title={t('Sign out')}
              icon="IconLogOut"
              disabled={!!busy}
              onPress={() => void signOutHere(() => signOut(), { confirmed: true })}
            />
          ) : (
            <Button title={t("I'm new here")} busy={busy === 'keep'} disabled={!!busy} onPress={() => void keepNew()} />
          )}
          {caught.mustFold ? (
            <Pressable disabled={!!busy} onPress={() => router.push('/delete-account')} style={styles.link} accessibilityRole="button">
              <Text style={styles.linkText}>{t('Delete this account instead')}</Text>
            </Pressable>
          ) : null}
        </>
      }>
      <Text style={styles.words}>{caught.words}</Text>
      {problem ? <Text style={styles.problem}>{problem}</Text> : null}
    </PromptScreen>
  );
}

/** Above the sign-in buttons, while a fold waits for the person's other account. */
export function FoldWaiting() {
  const styles = useStyles(makeStyles);
  const t = useT();
  const waiting = useWaitingFold();
  if (!waiting) return null;
  return (
    <View style={styles.waiting}>
      <Text style={styles.waitingText}>{waiting.waitingWords}</Text>
      <Pressable onPress={dropFold} accessibilityRole="button" hitSlop={8}>
        <Text style={styles.cancel}>{t('Cancel')}</Text>
      </Pressable>
    </View>
  );
}

type Read =
  | { state: 'loading' }
  | { state: 'failed'; problem: string; sameAccount: boolean }
  | { state: 'loaded'; preview: FoldPreview }
  | { state: 'done'; title: string; words: string };

function ConfirmFold({ waiting }: { waiting: Waiting }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  const { getToken, signOut, userId } = useAuth();
  const recent = useRecentSignIn(t(ELSEWHERE));
  const [read, setRead] = useState<Read>({ state: 'loading' });
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string>();
  const seatComing = waiting.why === 'seat';

  useEffect(() => {
    let current = true;
    void (async () => {
      try {
        const preview = await api(() => getToken()).previewFold(waiting.id, { secret: waiting.secret });
        if (current) setRead({ state: 'loaded', preview });
      } catch (error) {
        if (!current) return;
        if (error instanceof ApiProblem && error.status === 404) dropFold();
        setRead({ state: 'failed', problem: problemOf(error), sameAccount: errorCode(error) === 'same_account' });
      }
    })();
    return () => {
      current = false;
    };
  }, [getToken, waiting]);

  const cancel = () => {
    dropFold();
    checkFoldAgain();
  };
  const signInAgain = () => void signOutHere(() => signOut(), { confirmed: true });

  if (recent.verify)
    return (
      <ConfirmItsYou
        verify={recent.verify}
        onEnter={recent.enter}
        onCheck={(entry) => void recent.check(entry)}
        onBack={recent.cancel}
        action={{ title: seatComing ? t('Move the seat') : t('Bring it here'), kind: 'primary' }}
      />
    );

  if (read.state === 'loading') return <Checking />;

  if (read.state === 'failed')
    return (
      <PromptScreen
        icon="IconCircleAlert"
        title={t('Nothing was brought over')}
        detail={read.problem}
        actions={
          read.sameAccount ? (
            <>
              <Button kind="primary" title={t('Sign in with another account')} onPress={signInAgain} />
              <Button title={t('Cancel')} onPress={cancel} />
            </>
          ) : (
            <Button kind="primary" title={t('Continue')} onPress={cancel} />
          )
        }
      />
    );

  if (read.state === 'done')
    return (
      <PromptScreen
        icon="IconCircleCheck"
        title={read.title}
        detail={read.words}
        actions={
          <Button
            kind="primary"
            title={seatComing ? t('Sign in') : t('Continue')}
            onPress={() => (seatComing ? signInAgain() : userId ? void openFold(userId) : undefined)}
          />
        }
      />
    );

  const { preview } = read;
  const bring = async () => {
    setBusy(true);
    setProblem(undefined);
    try {
      await recent.run(() => api(() => getToken({ skipCache: true })).confirmFold(waiting.id, { secret: waiting.secret }));
      dropFold();
      setRead({ state: 'done', title: preview.doneTitle, words: preview.doneWords });
    } catch (error) {
      setBusy(false);
      if (!(error instanceof ReverifyCancelled)) setProblem(problemOf(error));
    }
  };

  return (
    <PromptScreen
      icon={seatComing ? 'IconMove' : 'IconShield'}
      title={preview.title}
      detail={<Text style={styles.ink}>{preview.address}</Text>}
      actions={
        <>
          <Button
            kind="primary"
            title={seatComing ? t('Move the seat') : t('Bring it here')}
            busy={busy}
            disabled={busy}
            onPress={() => void bring()}
          />
          <Button title={t('Cancel')} disabled={busy} onPress={cancel} />
        </>
      }>
      <Text style={styles.words}>{preview.words}</Text>
      {problem ? <Text style={styles.problem}>{problem}</Text> : null}
    </PromptScreen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    ink: { color: colors.ink },
    words: { ...type.body, textAlign: 'center', marginTop: 12 },
    problem: { ...type.meta, color: colors.danger, textAlign: 'center', marginTop: 12 },
    link: { height: 44, alignItems: 'center', justifyContent: 'center' },
    linkText: { ...type.meta, color: colors.secondary },
    waiting: { gap: 6, paddingBottom: 4 },
    waitingText: { ...type.body, textAlign: 'center' },
    cancel: { ...type.meta, color: colors.secondary, textAlign: 'center' },
  });
};
