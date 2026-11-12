import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { AccountMove, AgentNotice, ChatPart, TaskNotice as Notice } from '@shared/wire.generated';
import { useT, type Translate } from '@/i18n';
import { AgentIcon, Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';
import { useFold } from './folds';
import { GoalLine } from './GoalLine';
import { OverruledLine, WeekLine } from './ChiefLines';

function stateWord(state: Notice['state'], t: Translate): string {
  if (state === 'completed') return t('completed');
  if (state === 'failed') return t('failed');
  return t('stopped');
}

export function TaskNotice({ id, notice }: { id: string; notice: Notice }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [open, setOpen] = useFold(`notice/${id}`, false);
  const summary = notice.summary?.trim();
  const state = stateWord(notice.state, t);
  return (
    <Pressable
      onPress={summary ? () => setOpen(!open) : undefined}
      disabled={!summary}
      style={styles.notice}
      accessibilityRole={summary ? 'button' : undefined}
      accessibilityLabel={`${notice.name} ${state}${summary ? `. ${summary}` : ''}`}
      accessibilityState={summary ? { expanded: open } : undefined}>
      <View style={styles.noticeHead}>
        <Icon name={notice.taskType === 'workflow' ? 'IconWorkflow' : 'IconTimer'} size={12} color={colors.inkDim} />
        <Text style={styles.noticeName} numberOfLines={1}>
          {notice.name}
        </Text>
        <Text style={[styles.noticeState, notice.state === 'failed' && { color: colors.danger }]}>{state}</Text>
        {summary && !open ? (
          <Text style={styles.noticeInline} numberOfLines={1}>
            {summary}
          </Text>
        ) : null}
      </View>
      {summary && open ? (
        <Text style={styles.noticeSummary} selectable>
          {summary}
        </Text>
      ) : null}
    </Pressable>
  );
}

export function AgentNoticeLine({ notice }: { notice: AgentNotice }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  if (notice.goal) return <GoalLine notice={notice} goal={notice.goal} />;
  if (notice.overruled) return <OverruledLine overruled={notice.overruled} />;
  if (notice.week) return <WeekLine week={notice.week} />;
  const tint = notice.severity === 'error' ? colors.danger : notice.severity === 'warning' ? colors.warn : colors.inkDim;
  return (
    <View style={styles.notice} accessible accessibilityLabel={`${notice.title}${notice.description ? `. ${notice.description}` : ''}`}>
      <View style={styles.noticeHead}>
        <Icon name={notice.severity === 'info' ? 'IconInbox' : 'IconWarning'} size={12} color={tint} />
        <Text style={[styles.noticeState, notice.severity === 'error' && { color: tint }]}>{notice.title}</Text>
        {notice.description ? (
          <Text style={styles.noticeInline} numberOfLines={1}>
            {notice.description}
          </Text>
        ) : null}
      </View>
    </View>
  );
}

export function AccountLine({ move }: { move: AccountMove }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const why = move.reason === 'limit' && move.from ? t('{from} reached its usage limit', { from: move.from }) : null;
  const said = t('Moved to');
  return (
    <View
      style={styles.notice}
      accessible
      accessibilityLabel={`${t('Moved to {account}', { account: move.label })}${why ? `. ${why}` : ''}`}>
      <View style={styles.noticeHead}>
        <Icon name="IconUser" size={12} color={colors.inkDim} />
        <Text style={styles.noticeState}>{said}</Text>
        <Text style={[styles.noticeName, styles.noticeWords]} numberOfLines={1}>
          {move.label}
        </Text>
        {why ? (
          <Text style={styles.noticeInline} numberOfLines={1}>
            {why}
          </Text>
        ) : null}
      </View>
    </View>
  );
}

/** The account smart accounts picked for a chat about to start, and why, in one quiet centred line. */
export function SmartPick({ move, provider }: { move: AccountMove; provider: string }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <View
      style={styles.pick}
      accessible
      accessibilityLabel={`${t('Smart pick: {account}', { account: move.label })}${move.placed ? `. ${move.placed}` : ''}`}>
      <View style={styles.pickHead}>
        <AgentIcon provider={provider} size={12} />
        <Text style={styles.pickName} numberOfLines={1}>
          {move.label}
        </Text>
        <Text style={styles.pickCaps}>{t('Smart pick')}</Text>
      </View>
      {move.placed ? <Text style={styles.pickWhy}>{move.placed}</Text> : null}
    </View>
  );
}

type PageRef = NonNullable<Extract<ChatPart, { kind: 'tool' }>['page']>;

/** A page the agent showed. The phone has no web view to draw it in, so it names the page and where it is. */
export function PageCard({ page }: { page: PageRef }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <View style={styles.resource} accessible accessibilityLabel={t('Page {title}, open on the computer', { title: page.title })}>
      <Icon name="IconWindow" size={14} color={colors.inkDim} />
      <Text style={styles.pageTitle} numberOfLines={1}>
        {page.title}
      </Text>
      <Text style={styles.pageWhere}>{t('on the computer')}</Text>
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
    notice: { gap: 2, marginVertical: 2, paddingVertical: 4, paddingLeft: 12 },
    noticeHead: { flexDirection: 'row', alignItems: 'center', gap: 8, minHeight: 18 },
    pick: { alignItems: 'center', gap: 4, paddingTop: 40, paddingHorizontal: 16 },
    pickHead: { flexDirection: 'row', alignItems: 'center', gap: 7 },
    pickName: { flexShrink: 1, fontFamily: fonts.uiMedium, fontSize: 12.5, color: colors.ink },
    pickCaps: { fontFamily: fonts.uiMedium, fontSize: 10, letterSpacing: 1.6, textTransform: 'uppercase', color: colors.tertiary },
    pickWhy: { fontFamily: fonts.ui, fontSize: 12, color: colors.inkFaint, textAlign: 'center' },
    noticeName: { flexShrink: 1, fontFamily: fonts.mono, fontSize: 12, color: colors.ink },
    noticeWords: { fontFamily: fonts.ui, fontSize: 12.5 },
    noticeState: { flexShrink: 0, fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    noticeInline: { flexShrink: 1, minWidth: 0, fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    noticeSummary: { paddingLeft: 20, fontFamily: fonts.ui, fontSize: 12.5, lineHeight: 18, color: colors.inkDim },
  });
};
