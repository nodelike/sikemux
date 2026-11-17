import { useContext, useState } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { HIDE, leftOutCount, leftOutLine, SHOW } from '@shared/chat/leftOut';
import { externalValue, useExternalValue } from '@shared/lib/externalValue';
import type { ChatMessage, PartTree } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { connectionOf, liveOf } from '@/devices/hub';
import { HostContext } from '@/chat/hostFiles';
import { AgentContext } from '@/chat/transcriptContext';
import { useT } from '@/i18n';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** Each text part's tree with its left-out sentences back in place, by part id. */
export type Restored = ReadonlyMap<string, PartTree>;

/** The answers shown with what was left out of them, by chat and message, so a row scrolled away keeps them. */
const shown = externalValue<ReadonlyMap<string, Restored>>(new Map());
const keyOf = (agentId: string, messageId: string) => `${agentId}/${messageId}`;

/** Lets go of what chat `agentId` showed, once its screen closes. */
export function forgetShown(agentId: string) {
  shown.set(new Map([...shown.get()].filter(([key]) => !key.startsWith(`${agentId}/`))));
}

function put(key: string, restored: Restored | null) {
  const next = new Map(shown.get());
  if (restored) next.set(key, restored);
  else next.delete(key);
  shown.set(next);
}

/** What was left out of answer `messageId` in `agentId`'s chat, put back in place, while it is shown. */
export function useRestored(agentId: string, messageId: string): Restored | null {
  return useExternalValue(shown, (all) => all.get(keyOf(agentId, messageId)) ?? null);
}

/**
 * The quiet line under an answer some of whose sentences rested on no source and were left out, as the Mac says it.
 * Show asks the host for the answer's parts with those sentences back where they stood, drawn faint; Hide takes them
 * out again. When the host cannot put them back, for an answer too big for one page, a refusal or no connection, Show
 * lists them under the answer instead, faint, as the share page does.
 */
export function LeftOut({ message }: { message: ChatMessage }) {
  const colors = useColors();
  const t = useT();
  const styles = useStyles(makeStyles);
  const core = useContext(HostContext);
  const agentId = useContext(AgentContext);
  const key = keyOf(agentId, message.id);
  const restored = useRestored(agentId, message.id);
  const [asking, setAsking] = useState(false);
  const count = leftOutCount(message);
  if (!count) return null;
  const toggle = () => {
    if (restored) return put(key, null);
    if (asking) return;
    const connection = connectionOf(liveOf(core));
    if (!connection) return put(key, new Map());
    setAsking(true);
    callHost(connection, 'chatLeftOut', { agentId, messageId: message.id })
      .then((answer) => put(key, new Map(answer.parts.map((part) => [part.part, part.md]))))
      .catch((error: unknown) => {
        console.warn('sikemux: the host did not put back what it left out', problem(error));
        put(key, new Map());
      })
      .finally(() => setAsking(false));
  };
  return (
    <View style={styles.wrap}>
      <View style={styles.line}>
        <Text style={styles.words}>{leftOutLine(count, t)}</Text>
        <Text style={styles.words}>·</Text>
        <Pressable onPress={toggle} hitSlop={10} accessibilityRole="button" accessibilityState={{ expanded: !!restored }}>
          <Text style={[styles.words, { color: colors.secondary }]}>{t(restored ? HIDE : SHOW)}</Text>
        </Pressable>
      </View>
      {restored?.size === 0
        ? (message.leftOut ?? []).map((out) => (
            <Text key={`${out.part}:${out.at}`} style={styles.listed}>
              {out.text.trim()}
            </Text>
          ))
        : null}
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    wrap: { marginTop: 6, gap: 4 },
    line: { flexDirection: 'row', alignItems: 'center', gap: 6 },
    words: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    listed: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 24.5, color: colors.tertiary, marginTop: 4 },
  });
