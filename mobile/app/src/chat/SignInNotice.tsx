import { useContext, useState, type ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { NOT_CONNECTED } from '@shared/calls/words';
import type { SignInCard, SignInConnector } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { connectionOf, liveOf } from '@/devices/hub';
import { HostContext } from '@/chat/hostFiles';
import { ActsContext } from '@/chat/transcriptContext';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { ICONS, type IconName } from '@/ui/icons.generated';
import { fonts, type Palette, translucent, useColors, useStyles } from '@/ui/theme';

function namesOf(connectors: SignInConnector[]): string {
  return connectors.map((connector) => connector.name).join(', ');
}

/** The connector's mark, by its id, as `sikemux.jira` is `IconJira`. */
function markOf(id: string): IconName {
  const name = id.replace('sikemux.', '');
  const icon = `Icon${name.charAt(0).toUpperCase()}${name.slice(1)}`;
  return icon in ICONS ? (icon as IconName) : 'IconPlug';
}

/**
 * An agent's ask to sign in to connectors again, a row for each, which folds to one line once every one is back or it
 * was dismissed. The sign-in itself runs on the Mac.
 */
export function SignInNotice({ card }: { card: SignInCard }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const core = useContext(HostContext);
  const acts = useContext(ActsContext);
  const [sending, setSending] = useState(false);
  const [refused, setRefused] = useState<string>();
  const left = card.connectors.filter((connector) => connector.state.kind !== 'signedIn');

  if (left.length === 0) {
    return (
      <Settled>
        <Icon name="IconCheck" size={12} color={colors.live} />
        <Text style={[styles.text, styles.shrinks]} numberOfLines={1}>
          {t('Signed in to {names}', { names: namesOf(card.connectors) })}
        </Text>
        <Text style={[styles.text, styles.quiet]}>{t('· @{name} carried on', { name: card.name })}</Text>
      </Settled>
    );
  }
  if (card.skipped) {
    return (
      <Settled>
        <Icon name="IconLock" size={12} color={colors.inkDim} />
        <Text style={[styles.text, styles.quiet, styles.shrinks]} numberOfLines={1}>
          {t('Did not sign in to {names}', { names: namesOf(left) })}
        </Text>
      </Settled>
    );
  }

  const dismiss = () => {
    if (sending) return;
    const connection = connectionOf(liveOf(core));
    if (!connection) return setRefused(t(NOT_CONNECTED));
    haptics.select();
    setSending(true);
    setRefused(undefined);
    callHost(connection, 'dismissSignIn', { id: card.id })
      .catch((error: unknown) => setRefused(problem(error)))
      .finally(() => setSending(false));
  };
  return (
    <View style={styles.card}>
      <View style={styles.head}>
        <Icon name="IconLock" size={12} color={colors.inkDim} />
        <Text style={styles.text}>{t('@{name} needs you signed in', { name: card.name })}</Text>
        <Text style={[styles.text, styles.quiet, styles.why]} numberOfLines={1}>
          {card.why}
        </Text>
        {acts ? (
          <Pressable
            onPress={dismiss}
            disabled={sending}
            hitSlop={10}
            style={({ pressed }) => [styles.close, pressed && styles.pressed, sending && styles.sending]}
            accessibilityRole="button"
            accessibilityLabel={t('Dismiss')}>
            <Icon name="IconClose" size={12} color={colors.inkDim} />
          </Pressable>
        ) : null}
      </View>
      {refused ? <Text style={[styles.text, styles.failed, styles.refused]}>{refused}</Text> : null}
      {card.connectors.map((connector) => (
        <Row key={connector.id} connector={connector} />
      ))}
    </View>
  );
}

function Settled({ children }: { children: ReactNode }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.settled} accessible>
      {children}
    </View>
  );
}

function Row({ connector }: { connector: SignInConnector }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const { state } = connector;
  return (
    <View style={styles.row} accessible>
      <View style={styles.tile}>
        <Icon name={markOf(connector.id)} size={16} color={colors.secondary} />
      </View>
      <View style={styles.words}>
        <Text style={styles.text} numberOfLines={1}>
          {connector.name}
        </Text>
        <Text style={[styles.text, styles.quiet, state.kind === 'failed' && styles.failed]} numberOfLines={1}>
          {state.kind === 'failed' ? state.reason : state.kind === 'signedIn' ? t('Signed in') : t('Not signed in')}
        </Text>
      </View>
      {state.kind === 'signedIn' ? <Icon name="IconCheck" size={14} color={colors.live} /> : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    settled: { flexDirection: 'row', alignItems: 'center', gap: 8, minHeight: 18, marginVertical: 2, paddingVertical: 4, paddingLeft: 12 },
    text: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.ink },
    quiet: { color: colors.inkDim },
    failed: { color: colors.danger },
    shrinks: { flexShrink: 1, minWidth: 0 },
    card: {
      alignSelf: 'stretch',
      marginVertical: 6,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    head: { flexDirection: 'row', alignItems: 'center', gap: 8, paddingHorizontal: 12, paddingVertical: 8 },
    why: { flex: 1, minWidth: 0 },
    close: { width: 24, height: 24, borderRadius: 6, alignItems: 'center', justifyContent: 'center' },
    pressed: { backgroundColor: translucent(colors.ink, 0.06) },
    sending: { opacity: 0.5 },
    refused: { paddingHorizontal: 12, paddingBottom: 8 },
    row: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 12,
      paddingHorizontal: 12,
      paddingVertical: 8,
      borderTopWidth: 1,
      borderTopColor: colors.border,
    },
    tile: {
      width: 28,
      height: 28,
      alignItems: 'center',
      justifyContent: 'center',
      borderRadius: 6,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.active,
    },
    words: { flex: 1, minWidth: 0 },
  });
};
