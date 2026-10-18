import { useEffect, useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import { useAuth } from '@clerk/expo';
import type { PendingHost } from '@protocol';

import { platformName } from '@mac/remote/access';
import { approveHost, pendingHost, refuseHost } from '@/account/api';
import { retryHosts } from '@/account/session';
import { haptics } from '@/ui/haptics';
import { Button, Field } from '@/ui/controls';
import { goBack } from '@/ui/navigate';
import { Prompt } from '@/ui/prompt';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { type Palette, typeFor, useStyles } from '@/ui/theme';

type Step = { kind: 'enter' } | { kind: 'check'; code: string; host: PendingHost } | { kind: 'done'; name: string; added: boolean };

/** Signs in a host with no browser: the person types the code `sikemux login` shows there, checks the host and adds it. */
export default function HostSignIn() {
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  const { getToken } = useAuth();
  const { code: given } = useLocalSearchParams<{ code?: string }>();
  const [typed, setTyped] = useState(given ?? '');
  const [step, setStep] = useState<Step>({ kind: 'enter' });
  const [busy, setBusy] = useState<'look' | 'add' | 'refuse' | undefined>(given ? 'look' : undefined);
  const [problem, setProblem] = useState<string>();
  const token = () => getToken();

  const run = async (action: 'look' | 'add' | 'refuse', work: () => Promise<Step>) => {
    setBusy(action);
    setProblem(undefined);
    try {
      setStep(await work());
      haptics.success();
    } catch (error) {
      haptics.failure();
      setProblem(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(undefined);
    }
  };
  const look = (code: string) => run('look', async () => ({ kind: 'check', code, host: await pendingHost(token, code) }));

  useEffect(() => {
    if (!given) return;
    let current = true;
    pendingHost(() => getToken(), given)
      .then(
        (host) => current && setStep({ kind: 'check', code: given, host }),
        (error: unknown) => current && setProblem(error instanceof Error ? error.message : String(error)),
      )
      .finally(() => current && setBusy(undefined));
    return () => {
      current = false;
    };
  }, [given, getToken]);

  if (step.kind === 'done')
    return (
      <Screen>
        <Nav back="Devices" />
        <Prompt
          icon={step.added ? 'IconCheck' : 'IconClose'}
          title={step.added ? `${step.name} is on your account` : 'Refused'}
          detail={step.added ? undefined : `${step.name} was not added.`}
        />
        <View style={styles.form}>
          <Button title="Done" onPress={goBack} />
        </View>
      </Screen>
    );

  if (step.kind === 'check') {
    const { code, host } = step;
    return (
      <Screen>
        <Nav back="Back" onBack={() => setStep({ kind: 'enter' })} />
        <ScrollView contentContainerStyle={{ paddingBottom: bottom }}>
          <Prompt
            icon={host.platform === 'linux' ? 'desktop' : 'laptop'}
            title={`Add ${host.name} to your account?`}
            detail={`${platformName(host.platform)} · ${host.channel}. Only add a computer you are setting up right now.`}
          />
          <View style={styles.form}>
            {problem ? <Text style={styles.problem}>{problem}</Text> : null}
            <Button
              kind="primary"
              title="Add host"
              busy={busy === 'add'}
              disabled={busy !== undefined}
              onPress={() =>
                void run('add', async () => {
                  await approveHost(token, code);
                  retryHosts();
                  return { kind: 'done', name: host.name, added: true };
                })
              }
            />
            <Button
              kind="danger"
              title="Not mine"
              busy={busy === 'refuse'}
              disabled={busy !== undefined}
              onPress={() =>
                void run('refuse', async () => {
                  await refuseHost(token, code);
                  return { kind: 'done', name: host.name, added: false };
                })
              }
            />
          </View>
        </ScrollView>
      </Screen>
    );
  }

  const entered = typed.trim();
  return (
    <Screen>
      <Nav back="Devices" />
      <ScrollView contentContainerStyle={{ paddingBottom: bottom }} keyboardShouldPersistTaps="handled">
        <Prompt
          icon="IconServer"
          title="Add a host with a code"
          detail={
            <>
              Enter the code <Text style={styles.mono}>sikemux login</Text> shows on the computer.
            </>
          }
        />
        <View style={styles.form}>
          <Field
            icon="IconTerminal"
            value={typed}
            onChangeText={setTyped}
            placeholder="BCDF-GHJK"
            autoCapitalize="characters"
            autoCorrect={false}
            autoComplete="off"
            autoFocus={!given}
            returnKeyType="go"
            onSubmitEditing={() => entered && void look(entered)}
            accessibilityLabel="Code"
            style={styles.code}
          />
          {problem ? <Text style={styles.problem}>{problem}</Text> : null}
          <Button
            kind="primary"
            title="Continue"
            busy={busy === 'look'}
            disabled={!entered || busy !== undefined}
            onPress={() => void look(entered)}
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
    mono: { ...type.mono, color: colors.ink },
    code: { ...type.mono, fontSize: 20, letterSpacing: 2, color: colors.ink },
    problem: { ...type.meta, fontSize: 13.5, color: colors.danger },
  });
};
