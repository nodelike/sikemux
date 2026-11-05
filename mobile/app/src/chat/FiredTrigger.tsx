import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { ChatMessage } from '@shared/chat/types';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { useFold } from './folds';
import { LongText } from '@/chat/LongText';

type Fired = NonNullable<NonNullable<ChatMessage['letters']>[number]['trigger']>;

/** A trigger's prompt where the person's own messages sit, under a line naming the trigger, with the
 *  webhook request that fired it folded beneath. */
export function FiredTrigger({ id, fired }: { id: string; fired: Fired }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [open, setOpen] = useFold(`${id}/request`, false);
  const request = fired.request;
  const kind = request?.headers
    .find((header) => header.name === 'content-type')
    ?.value.split(';')[0]
    ?.trim();
  const size = request ? (request.body.length < 1024 ? `${request.body.length} B` : `${(request.body.length / 1024).toFixed(1)} KB`) : '';
  const summary = request ? [request.method, kind, size, request.query ? `?${request.query}` : undefined].filter(Boolean).join(' · ') : '';
  return (
    <View style={styles.userRow}>
      <View style={styles.triggerFrom}>
        <Icon name="IconTimer" size={12} color={colors.cmd} />
        <Text style={styles.letterLabel} numberOfLines={1}>
          Trigger · <Text style={styles.triggerTitle}>{fired.title}</Text>
        </Text>
      </View>
      <View style={styles.bubble}>
        <Text style={styles.userText} selectable>
          {fired.prompt}
        </Text>
      </View>
      {request ? (
        <>
          <Pressable
            onPress={() => setOpen(!open)}
            style={styles.requestLine}
            accessibilityRole="button"
            accessibilityState={{ expanded: open }}
            accessibilityLabel={`The request that fired it: ${summary}`}>
            <View style={open ? { transform: [{ rotate: '90deg' }] } : undefined}>
              <Icon name="IconChevron" size={10} color={colors.inkFaint} />
            </View>
            <Text style={styles.requestSummary} numberOfLines={1}>
              {summary}
            </Text>
          </Pressable>
          {open ? (
            <View style={styles.requestCard}>
              <View>
                {request.headers.map((header) => (
                  <Text key={header.name} style={styles.requestHeader} selectable>
                    <Text style={styles.requestName}>{header.name}</Text> {header.value}
                  </Text>
                ))}
              </View>
              {request.bodyTree ? (
                <LongText id={`${id}/request/body`} text={request.body} md={request.bodyTree} live={false} style={styles.prose} />
              ) : null}
            </View>
          ) : null}
        </>
      ) : null}
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
    prose: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 23, color: colors.ink },
    letterLabel: { fontFamily: fonts.ui, fontSize: 11.5, lineHeight: 16, color: colors.inkDim },
    triggerFrom: { flexDirection: 'row', alignItems: 'center', gap: 5, marginBottom: 6, maxWidth: '85%' },
    triggerTitle: { fontFamily: fonts.uiMedium, color: colors.ink, flexShrink: 1 },
    requestLine: { flexDirection: 'row', alignItems: 'center', gap: 6, marginTop: 6, paddingVertical: 4, paddingHorizontal: 8 },
    requestSummary: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkFaint, flexShrink: 1 },
    requestCard: {
      alignSelf: 'stretch',
      marginTop: 4,
      padding: 10,
      gap: 8,
      borderWidth: StyleSheet.hairlineWidth,
      borderColor: colors.border,
      borderRadius: 8,
      backgroundColor: colors.raised,
    },
    requestHeader: { fontFamily: fonts.mono, fontSize: 11, lineHeight: 17, color: colors.ink },
    requestName: { color: colors.inkFaint },
  });
};
