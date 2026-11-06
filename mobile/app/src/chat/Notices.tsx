import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { AccountMove, AgentNotice, ChatPart, TaskNotice as Notice } from '@shared/wire.generated';
import { Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { useFold } from './folds';

export function TaskNotice({ id, notice }: { id: string; notice: Notice }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [open, setOpen] = useFold(`notice/${id}`, false);
  const summary = notice.summary?.trim();
  return (
    <Pressable
      onPress={summary ? () => setOpen(!open) : undefined}
      disabled={!summary}
      style={styles.notice}
      accessibilityRole={summary ? 'button' : undefined}
      accessibilityLabel={`${notice.name} ${notice.state}${summary ? `. ${summary}` : ''}`}
      accessibilityState={summary ? { expanded: open } : undefined}>
      <View style={styles.noticeHead}>
        <Icon
          name={notice.taskType === 'workflow' ? 'IconWorkflow' : 'IconTimer'}
          size={12}
          color={notice.state === 'failed' ? colors.danger : colors.inkFaint}
        />
        <Text style={styles.noticeName} numberOfLines={1}>
          {notice.name}
        </Text>
        <Text style={[styles.noticeState, notice.state === 'failed' && { color: colors.danger }]}>{notice.state}</Text>
      </View>
      {summary ? (
        <Text style={styles.noticeSummary} numberOfLines={open ? undefined : 1} selectable={open}>
          {summary}
        </Text>
      ) : null}
    </Pressable>
  );
}

export function AgentNoticeLine({ notice }: { notice: AgentNotice }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const tint = notice.severity === 'error' ? colors.danger : notice.severity === 'warning' ? colors.warn : colors.inkFaint;
  return (
    <View style={styles.notice} accessible accessibilityLabel={`${notice.title}${notice.description ? `. ${notice.description}` : ''}`}>
      <View style={styles.noticeHead}>
        <Icon name={notice.severity === 'info' ? 'IconInfo' : 'IconWarning'} size={12} color={tint} />
        <Text style={[styles.noticeState, notice.severity !== 'info' && { color: tint }]}>{notice.title}</Text>
      </View>
      {notice.description ? (
        <Text style={styles.noticeSummary} selectable>
          {notice.description}
        </Text>
      ) : null}
    </View>
  );
}

export function AccountLine({ move }: { move: AccountMove }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const why = move.reason === 'limit' && move.from ? `${move.from} reached its usage limit` : null;
  return (
    <View style={styles.notice} accessible accessibilityLabel={`Moved to ${move.label}${why ? `. ${why}` : ''}`}>
      <View style={styles.noticeHead}>
        <Icon name="IconUser" size={12} color={colors.inkFaint} />
        <Text style={styles.noticeState}>Moved to</Text>
        <Text style={styles.noticeName} numberOfLines={1}>
          {move.label}
        </Text>
      </View>
      {why ? <Text style={styles.noticeSummary}>{why}</Text> : null}
    </View>
  );
}

type PageRef = NonNullable<Extract<ChatPart, { kind: 'tool' }>['page']>;

/** A page the agent showed. The phone has no web view to draw it in, so it names the page and where it is. */
export function PageCard({ page }: { page: PageRef }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.resource} accessible accessibilityLabel={`Page ${page.title}, open on the computer`}>
      <Icon name="IconWindow" size={14} color={colors.inkDim} />
      <Text style={styles.pageTitle} numberOfLines={1}>
        {page.title}
      </Text>
      <Text style={styles.pageWhere}>on the computer</Text>
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    resource: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      alignSelf: 'flex-start',
      maxWidth: '100%',
      marginVertical: 6,
      paddingVertical: 7,
      paddingHorizontal: 10,
      borderRadius: 8,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.raised,
    },
    pageTitle: { flexShrink: 1, fontFamily: fonts.uiMedium, fontSize: 13, color: colors.ink },
    pageWhere: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    notice: {
      alignSelf: 'flex-start',
      maxWidth: '100%',
      gap: 3,
      marginVertical: 5,
      paddingVertical: 6,
      paddingHorizontal: 10,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
    },
    noticeHead: { flexDirection: 'row', alignItems: 'center', gap: 7 },
    noticeName: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 11.5, color: colors.ink },
    noticeState: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint },
    noticeSummary: { fontFamily: fonts.ui, fontSize: 12, lineHeight: 17, color: colors.inkFaint },
  });
};
