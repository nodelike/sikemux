import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { ChatMessage } from '@shared/wire.generated';
import { useT } from '@/i18n';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { useFold } from './folds';
import { LongText } from '@/chat/LongText';
import { From, makeBubbleStyles } from './Bubble';

type Fired = NonNullable<NonNullable<ChatMessage['letters']>[number]['trigger']>;

/** A trigger's prompt in a bubble at the left under the trigger's name, with the webhook request that fired it
 *  folded beneath. */
export function FiredTrigger({ id, fired, at }: { id: string; fired: Fired; at?: number }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const bubble = useStyles(makeBubbleStyles);
  const t = useT();
  const [open, setOpen] = useFold(`${id}/request`, false);
  const request = fired.request;
  const kind = request?.headers
    .find((header) => header.name === 'content-type')
    ?.value.split(';')[0]
    ?.trim();
  const size = request ? (request.body.length < 1024 ? `${request.body.length} B` : `${(request.body.length / 1024).toFixed(1)} KB`) : '';
  const summary = request ? [request.method, kind, size, request.query ? `?${request.query}` : undefined].filter(Boolean).join(' · ') : '';
  return (
    <View>
      <From
        at={at}
        who={
          <>
            <Icon name="IconTimer" size={13} color={colors.cmd} />
            <Text style={styles.triggerTitle} numberOfLines={1}>
              {fired.title}
            </Text>
          </>
        }>
        <Text style={bubble.text} selectable>
          {fired.prompt}
        </Text>
      </From>
      {request ? (
        <>
          <Pressable
            onPress={() => setOpen(!open)}
            style={styles.requestLine}
            accessibilityRole="button"
            accessibilityState={{ expanded: open }}
            accessibilityLabel={t('The request that fired it: {summary}', { summary })}>
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
    prose: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 23, color: colors.ink },
    triggerTitle: { flexShrink: 1, fontFamily: fonts.uiSemibold, fontSize: 12.5, color: colors.ink },
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
