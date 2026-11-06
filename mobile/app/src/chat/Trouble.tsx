import { useEffect, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import type { ConnectionLike } from '@sikemux/native';

import type { ChatRows } from '@shared/chat/rows';
import { agentLabel, agentMaker } from '@shared/agents/catalog';
import { useHostCall } from '@/core/hostCalls';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import type { ChatSession } from './session';

/** Whether `at`, in Unix milliseconds, is still to come; it turns false as `at` passes. */
function useAhead(at: number | undefined) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (at === undefined || at <= now) return;
    const passing = setTimeout(() => setNow(Date.now()), at - now);
    return () => clearTimeout(passing);
  }, [at, now]);
  return at !== undefined && at > now;
}

function Action({ title, onPress }: { title: string; onPress: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={onPress}
      hitSlop={4}
      style={({ pressed }) => [styles.action, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="button">
      <Text style={styles.actionText}>{title}</Text>
    </Pressable>
  );
}

/**
 * What stands at the end of the chat when something went wrong, as on the Mac: that the session dropped, or why
 * the last turn failed, with what can be done about it from here. Signing in happens on the host.
 */
export function Trouble({
  state,
  provider,
  hostName,
  connection,
  session,
  canAct,
  onStartOver,
}: {
  state: Pick<ChatRows, 'connection' | 'error' | 'failure' | 'ended' | 'messages'>;
  provider: string;
  hostName: string;
  connection: ConnectionLike | undefined;
  session: ChatSession;
  canAct: boolean;
  onStartOver?: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const failure = state.failure;
  const accountsNeeded = canAct && (failure?.kind === 'signIn' || failure?.kind === 'limit');
  /** The account a chat runs on and the signed-in ones that keep its chats too, as the host's `chatAccounts` answers. */
  const accounts = useHostCall(
    accountsNeeded ? connection : undefined,
    'chatAccounts',
    { agentId: session.agentId },
    `${failure?.kind}|${failure?.account}`,
  ).data;
  const resumes = useAhead(failure?.kind === 'limit' ? failure.resumesAt : undefined);
  const dropped = state.connection === 'error' || state.connection === 'stopped';
  if (dropped && state.messages.length) {
    const why = state.ended?.message;
    return (
      <View style={styles.trouble} accessibilityLiveRegion="polite">
        <View style={styles.said}>
          <View style={styles.glyph}>
            <Icon name="IconPlug" size={14} color={colors.danger} />
          </View>
          <Text style={styles.text}>{why ? `This session dropped: ${why}` : 'This session dropped.'}</Text>
        </View>
        {canAct ? (
          <View style={styles.actions}>
            <Action title="Reconnect" onPress={session.retry} />
            {onStartOver ? <Action title="Start new chat" onPress={onStartOver} /> : null}
          </View>
        ) : null}
      </View>
    );
  }
  if (!state.error) return null;
  const network = failure?.kind === 'network';
  const name = accounts?.account?.label ?? agentLabel(provider);
  const moves = failure?.kind === 'limit' ? (accounts?.others ?? []) : [];
  const signIn = failure?.kind === 'signIn';
  return (
    <View style={styles.trouble} accessibilityLiveRegion="polite">
      <View style={styles.said}>
        <View style={styles.glyph}>
          <Icon name="IconWarning" size={14} color={colors.danger} />
        </View>
        <Text style={styles.text} selectable>
          {network
            ? `Lost connection to ${agentMaker(provider) ?? 'the model provider'}. Sikemux sends it again when you're back online.`
            : state.error}
        </Text>
      </View>
      {network && failure?.details ? (
        <Text style={styles.details} selectable>
          {failure.details}
        </Text>
      ) : null}
      {signIn ? <Text style={styles.host}>{`Sign in to ${name} on ${hostName}, then retry here.`}</Text> : null}
      {canAct && (network || resumes || moves.length || signIn) ? (
        <View style={styles.actions}>
          {network ? <Action title="Retry now" onPress={() => session.failedTurn('retryFailed')} /> : null}
          {network ? <Action title="Discard" onPress={() => session.failedTurn('discardFailed')} /> : null}
          {resumes ? <Action title="Don't resume" onPress={() => session.failedTurn('cancelResume')} /> : null}
          {moves.map((other) => (
            <Action key={other.id} title={`Continue on ${other.label}`} onPress={() => session.moveToAccount(other.id)} />
          ))}
          {signIn ? <Action title="Retry" onPress={() => session.failedTurn('retryFailed')} /> : null}
        </View>
      ) : null}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    trouble: { gap: 8, marginTop: 10 },
    said: { flexDirection: 'row', alignItems: 'flex-start', gap: 8 },
    glyph: { height: 19, justifyContent: 'center' },
    text: { flex: 1, fontFamily: fonts.ui, fontSize: 13.5, lineHeight: 19, color: colors.secondary },
    details: { marginLeft: 22, fontFamily: fonts.mono, fontSize: 11.5, lineHeight: 16, color: colors.tertiary },
    host: { marginLeft: 22, fontFamily: fonts.ui, fontSize: 13, lineHeight: 18, color: colors.tertiary },
    actions: { flexDirection: 'row', flexWrap: 'wrap', gap: 8, marginLeft: 22 },
    action: {
      height: 32,
      paddingHorizontal: 13,
      borderRadius: 16,
      borderWidth: 1,
      borderColor: colors.border,
      justifyContent: 'center',
    },
    actionText: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.ink },
  });
