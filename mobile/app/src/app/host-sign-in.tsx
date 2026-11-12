import { useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';
import { useAuth } from '@clerk/expo';
import type { PendingHost } from '@sikemux/protocol';

import { formatAgo, formatDay } from '@shared/lib/format';
import { platformName } from '@shared/remote/access';
import { problem as problemOf } from '@/core/problem';
import { api } from '@/account/api';
import { ConfirmItsYou, ReverifyCancelled, useRecentSignIn } from '@/account/ConfirmItsYou';
import { retryHosts } from '@/account/session';
import { type Translate, useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { Button, Field } from '@/ui/controls';
import { Row, Rows } from '@/ui/list';
import { goBack } from '@/ui/navigate';
import { Prompt } from '@/ui/prompt';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { type Palette, typeFor, useColors, useStyles } from '@/ui/theme';

type Step = { kind: 'enter' } | { kind: 'check'; code: string; host: PendingHost } | { kind: 'done'; name: string; added: boolean };

function asked(at: string, t: Translate): string {
  const ago = formatAgo(at, Date.now(), t);
  return ago === t('now') ? t('Asked just now') : t('Asked {ago}', { ago });
}

/**
 * Signs in a host with no browser: the person types the code `sikemux login` shows there, checks when and
 * where the host asked from, and adds it after proving it is them. A link never fills the code in.
 */
export default function HostSignIn() {
  const styles = useStyles(makeStyles);
  const colors = useColors();
  const t = useT();
  const bottom = useBottomGap();
  const { getToken } = useAuth();
  const recent = useRecentSignIn(t('Add it at app.sikemux.com/host.'));
  const [typed, setTyped] = useState('');
  const [step, setStep] = useState<Step>({ kind: 'enter' });
  const [busy, setBusy] = useState<'look' | 'add' | 'refuse' | undefined>();
  const [problem, setProblem] = useState<string>();
  const token = () => getToken();

  const run = async (action: 'look' | 'add' | 'refuse', work: () => Promise<Step>) => {
    setBusy(action);
    setProblem(undefined);
    try {
      setStep(await work());
      haptics.success();
    } catch (error) {
      if (error instanceof ReverifyCancelled) return;
      haptics.failure();
      setProblem(problemOf(error));
    } finally {
      setBusy(undefined);
    }
  };
  const look = (code: string) => run('look', async () => ({ kind: 'check', code, host: await api(token).getPendingHost(code) }));

  if (step.kind === 'done')
    return (
      <Screen>
        <Nav back={t('Devices')} />
        <Prompt
          icon={step.added ? 'IconCheck' : 'IconClose'}
          title={step.added ? t('{name} is on your account', { name: step.name }) : t('Refused')}
          detail={step.added ? undefined : t('{name} was not added.', { name: step.name })}
        />
        <View style={styles.form}>
          <Button title={t('Done')} onPress={goBack} />
        </View>
      </Screen>
    );

  if (recent.verify)
    return (
      <ConfirmItsYou
        verify={recent.verify}
        onEnter={recent.enter}
        onCheck={(entry) => void recent.check(entry)}
        onBack={recent.cancel}
        action={{ title: t('Add host'), kind: 'primary' }}
      />
    );

  if (step.kind === 'check') {
    const { code, host } = step;
    const near = host.requestedNear ?? host.requestedFrom;
    return (
      <Screen>
        <Nav back={t('Back')} onBack={() => setStep({ kind: 'enter' })} />
        <ScrollView contentContainerStyle={{ paddingBottom: bottom }} showsVerticalScrollIndicator={false}>
          <Prompt
            icon={host.platform === 'linux' ? 'desktop' : 'laptop'}
            title={t('Add {name} to your account?', { name: host.name })}
            detail={t('{platform} · {channel}. Only add a computer you are setting up right now.', {
              platform: platformName(host.platform),
              channel: host.channel,
            })}
          />
          <View style={styles.form}>
            <Rows>
              <Row mark={<Icon name="IconClock" size={18} color={colors.secondary} />} title={asked(host.requestedAt, t)} />
              <Row
                mark={<Icon name="IconGlobe" size={18} color={colors.secondary} />}
                title={near ? t('From {place}', { place: near }) : t('From an unknown place')}
                detail={host.requestedNear && host.requestedFrom ? host.requestedFrom : undefined}
              />
            </Rows>
            {host.removedAt ? (
              <Text style={styles.warning}>
                {t('This host was removed from your account on {day}. Add it back only if you are setting it up again yourself.', {
                  day: formatDay(host.removedAt),
                })}
              </Text>
            ) : null}
            {problem ? <Text style={styles.problem}>{problem}</Text> : null}
            <Button
              kind="primary"
              title={t('Add host')}
              busy={busy === 'add'}
              disabled={busy !== undefined}
              onPress={() =>
                void run('add', async () => {
                  await recent.run(() => api(() => getToken({ skipCache: true })).approveHostSignIn(code));
                  retryHosts();
                  return { kind: 'done', name: host.name, added: true };
                })
              }
            />
            <Button
              kind="danger"
              title={t('Not mine')}
              busy={busy === 'refuse'}
              disabled={busy !== undefined}
              onPress={() =>
                void run('refuse', async () => {
                  await api(token).refuseHostSignIn(code);
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
  const [enterBefore, enterAfter = ''] = t('Enter the code {command} shows on the computer.').split('{command}');
  return (
    <Screen>
      <Nav back={t('Devices')} />
      <ScrollView
        contentContainerStyle={{ paddingBottom: bottom }}
        keyboardShouldPersistTaps="handled"
        showsVerticalScrollIndicator={false}>
        <Prompt
          icon="IconServer"
          title={t('Add a host with a code')}
          detail={
            <>
              {enterBefore}
              <Text style={styles.mono}>sikemux login</Text>
              {enterAfter}
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
            autoFocus
            returnKeyType="go"
            onSubmitEditing={() => entered && void look(entered)}
            accessibilityLabel={t('Code')}
            style={styles.code}
          />
          {problem ? <Text style={styles.problem}>{problem}</Text> : null}
          <Button
            kind="primary"
            title={t('Continue')}
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
    warning: { ...type.body, color: colors.danger },
  });
};
