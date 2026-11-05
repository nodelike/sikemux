import { Pressable, StyleSheet, Text, View } from 'react-native';

import { EARLIER_LOST } from '@shared/chat/messageMeta';
import { Working } from '@/devices/AgentMarks';
import { fonts, type Palette, useStyles } from '@/ui/theme';
import { SentAttachments } from './Attachments';
import type { Held } from './outbox';
import type { Attachment } from './uploads';

/** Messages waiting behind the running turn, faded where they will go. */
export function Queued({ held, sentFiles }: { held: readonly Held[]; sentFiles: ReadonlyMap<string, Attachment> }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.userRow}>
      {held.map((message) => {
        const { paths } = message;
        return (
          <View key={message.id} style={styles.queued}>
            {paths.length ? <SentAttachments paths={paths} sentFiles={sentFiles} /> : null}
            {message.text ? (
              <View style={styles.bubble}>
                <Text style={styles.userText}>{message.text}</Text>
              </View>
            ) : null}
          </View>
        );
      })}
      <Text style={styles.queuedLabel}>Sends when this turn ends</Text>
    </View>
  );
}

/** Above the first message while the host has turns from before it, which arrive as it comes into view. */
export function Earlier({ failed, loading, onRetry }: { failed: boolean; loading: boolean; onRetry: () => void }) {
  const styles = useStyles(makeStyles);
  if (failed) {
    return (
      <Pressable
        onPress={onRetry}
        style={styles.earlier}
        hitSlop={8}
        accessibilityRole="button"
        accessibilityLabel="Couldn't load earlier messages. Retry">
        <Text style={styles.earlierText}>{"Couldn't load earlier messages ·"}</Text>
        <Text style={[styles.earlierText, styles.retry]}>Retry</Text>
      </Pressable>
    );
  }
  // The row waits above a long chat's first message; an idle spinner there would redraw the screen every frame.
  return (
    <View style={styles.earlier}>
      {loading ? <Working /> : <View style={styles.earlierIdle} />}
      <Text style={styles.earlierText}>Loading earlier messages…</Text>
    </View>
  );
}

/** Stands above a chat whose host no longer keeps its first turns. */
export function EarlierLost() {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.earlier}>
      <Text style={styles.earlierText}>{EARLIER_LOST}</Text>
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    userRow: { alignItems: 'flex-end', marginTop: 14, marginBottom: 6 },
    bubble: {
      maxWidth: '84%',
      paddingVertical: 9,
      paddingHorizontal: 13,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    userText: { fontFamily: fonts.ui, fontSize: 14, lineHeight: 22, color: colors.ink },
    retry: { color: colors.ink, fontFamily: fonts.uiMedium },
    queued: { alignItems: 'flex-end', alignSelf: 'stretch', opacity: 0.55, marginBottom: 6 },
    queuedLabel: { fontFamily: fonts.ui, fontSize: 11, color: colors.inkFaint, marginTop: 4 },
    earlier: { flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 8, paddingTop: 10, paddingBottom: 6 },
    earlierIdle: { width: 13, height: 13 },
    earlierText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
  });
};
