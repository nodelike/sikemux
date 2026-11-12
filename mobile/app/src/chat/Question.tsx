import type { ReactNode } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import { assumed, CHANGE, FINE } from '@shared/calls/words';
import { useT } from '@/i18n';
import type { AgentQuestion } from '@shared/wire.generated';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { AgentFace } from '@/devices/Buddy';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

import { Bubble, makeBubbleStyles, TheirBubble } from './Bubble';

export type Answered = { question: string; answer: string; secret: boolean; files?: ReactNode };

/**
 * The questions an agent asked and the person's answers, as the conversation they were: each question in the
 * asker's bubble at the left, its mark on the first, and the answer in the person's bubble at the right.
 * Someone else's questions say who asked above them.
 */
export function AnsweredQuestions({
  agentId,
  cli,
  asker,
  answers,
}: {
  agentId: string;
  cli: string;
  /** Who asked, when it was not this chat's own agent. */
  asker: string | null;
  answers: readonly Answered[];
}) {
  const styles = useStyles(makeStyles);
  const bubble = useStyles(makeBubbleStyles);
  const t = useT();
  return (
    <View style={styles.thread}>
      {asker ? <Text style={styles.askedBy}>@{asker}</Text> : null}
      {answers.map((answered, index) => (
        <View key={index} style={styles.pair}>
          <View style={styles.asked}>
            <View style={index === 0 ? styles.askerMark : styles.askerGap}>
              {index === 0 ? <AgentFace agentId={agentId} provider={cli} size={ASKER_MARK} /> : null}
            </View>
            <TheirBubble>
              <Text style={bubble.text} selectable>
                {answered.question}
              </Text>
            </TheirBubble>
          </View>
          <View style={styles.said}>
            {answered.files}
            {answered.secret ? (
              <Bubble>
                <Text style={[bubble.text, styles.kept]}>{t('Kept private')}</Text>
              </Bubble>
            ) : answered.answer ? (
              <Bubble>
                <Text style={bubble.text} selectable>
                  {answered.answer}
                </Text>
              </Bubble>
            ) : null}
          </View>
        </View>
      ))}
    </View>
  );
}

const ASKER_MARK = 16;

/** A question the agent went on without, on the assumption it stated: Fine lets it stand, Change answers it. */
export function AssumedQuestion({
  question,
  canAnswer,
  onFine,
  onChange,
}: {
  question: AgentQuestion;
  canAnswer: boolean;
  onFine: () => void;
  onChange: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const act = (label: string, onPress: () => void) => (
    <Text
      style={styles.act}
      onPress={() => {
        haptics.select();
        onPress();
      }}
      accessibilityRole="button">
      {label}
    </Text>
  );
  return (
    <View style={styles.assumed}>
      <View style={styles.check}>
        <Icon name="IconCheck" size={13} color={colors.inkDim} />
      </View>
      <Text style={styles.assumedText}>
        <Text style={styles.name}>@{question.name}</Text> {assumed(question.assumption ?? '', t)}
        {canAnswer ? (
          <>
            {'  ·  '}
            {act(t(FINE), onFine)}
            {'   '}
            {act(t(CHANGE), onChange)}
          </>
        ) : null}
      </Text>
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    thread: { gap: 6, marginTop: 14, marginBottom: 6 },
    askedBy: { paddingLeft: ASKER_MARK + 10, fontFamily: fonts.ui, fontSize: 12, color: colors.tertiary },
    pair: { gap: 6 },
    asked: { maxWidth: '86%', flexDirection: 'row', alignItems: 'flex-start', gap: 10 },
    askerMark: { width: ASKER_MARK, marginTop: 13 },
    askerGap: { width: ASKER_MARK },
    said: { alignItems: 'flex-end', gap: 6, marginLeft: '14%' },
    kept: { color: colors.tertiary },
    who: { flexDirection: 'row', alignItems: 'center', gap: 7, marginTop: 14, marginBottom: 2 },
    whoText: { flexShrink: 1, fontFamily: fonts.ui, fontSize: 11.5, lineHeight: 16, color: colors.inkDim },
    name: { fontFamily: fonts.uiSemibold, color: colors.ink },
    prose: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 23, color: colors.ink },
    pickAny: { color: colors.inkFaint },
    options: { gap: 2, marginTop: 6, marginBottom: 4, marginHorizontal: -10 },
    option: { flexDirection: 'row', alignItems: 'flex-start', gap: 12, minHeight: 40, padding: 10, borderRadius: 10 },
    mark: {
      width: 14,
      height: 14,
      marginTop: 3,
      alignItems: 'center',
      justifyContent: 'center',
      borderWidth: 1.5,
      borderColor: colors.borderStrong,
    },
    radio: { borderRadius: 7 },
    tick: { borderRadius: 4 },
    markOn: { borderColor: colors.ink },
    dot: { width: 6, height: 6, borderRadius: 3, backgroundColor: colors.ink },
    pencil: { width: 14, height: 20, justifyContent: 'center' },
    label: { flex: 1, fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 20, color: colors.ink },
    recommended: { fontSize: 12, color: colors.inkFaint },
    why: { fontSize: 13, color: colors.inkDim },
    fold: { width: 24, height: 20, alignItems: 'center', justifyContent: 'center' },
    down: { transform: [{ rotate: '90deg' }] },
    tabs: { marginTop: 8, marginBottom: 6, marginHorizontal: -18, flexGrow: 0 },
    tabsRow: { gap: 6, paddingHorizontal: 18 },
    tab: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 6,
      height: 30,
      paddingHorizontal: 10,
      borderRadius: 9,
      borderWidth: 1,
      borderColor: colors.border,
    },
    tabOn: { backgroundColor: colors.active, borderColor: 'transparent' },
    tabNumber: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkFaint },
    tabText: { maxWidth: 180, fontFamily: fonts.ui, fontSize: 12.5, color: colors.secondary },
    send: { alignItems: 'flex-end', marginBottom: 8 },
    folded: { flexDirection: 'row', alignItems: 'center', gap: 8, marginTop: 14, marginBottom: 6, paddingVertical: 10 },
    foldedLabel: { fontFamily: fonts.uiMedium, fontSize: 11, letterSpacing: 0.6, color: colors.inkFaint },
    foldedCount: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkDim },
    foldedNames: { flex: 1, fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    ticket: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkDim, marginLeft: 'auto' },
    assumed: { flexDirection: 'row', alignItems: 'flex-start', gap: 7, marginTop: 10, marginBottom: 6 },
    check: { height: 18, justifyContent: 'center' },
    assumedText: { flex: 1, fontFamily: fonts.ui, fontSize: 12.5, lineHeight: 18, color: colors.inkDim },
    act: { fontFamily: fonts.uiMedium, color: colors.ink },
    secret: { marginTop: 8, marginBottom: 6, gap: 8 },
    secretField: { flexDirection: 'row', alignItems: 'flex-start', gap: 8 },
    secretInput: {
      flex: 1,
      height: 'auto',
      minHeight: 40,
      maxHeight: 140,
      paddingTop: 10,
      paddingBottom: 10,
      textAlignVertical: 'top',
      color: 'transparent',
    },
    secretSize: { fontFamily: fonts.ui, fontSize: 12.5, lineHeight: 18, color: colors.inkDim },
    private: { flexDirection: 'row', alignItems: 'flex-start', gap: 6, marginTop: 4, marginBottom: 8 },
    privateText: { flex: 1, fontFamily: fonts.ui, fontSize: 12.5, lineHeight: 18, color: colors.inkDim },
  });
