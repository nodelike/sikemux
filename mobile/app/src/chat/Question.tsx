import { useState, type ReactNode } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { SECRET_ANSWERED } from '@shared/chat/agentLetters';
import type { AgentQuestion } from '@shared/wire.generated';
import { Chip, Field } from '@/ui/controls';
import { haptics } from '@/ui/haptics';
import { AgentIcon, Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

import type { ChatQuestions } from './questions';

/** Who is asking, above the question itself, and the ticket of the board card it works; `onFold` adds the chevron that folds the questions away. */
function AskedBy({ cli, ticket, onFold, children }: { cli: string; ticket?: string | null; onFold?: () => void; children: ReactNode }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.who}>
      <AgentIcon provider={cli} size={14} />
      <Text style={styles.whoText} numberOfLines={1}>
        {children}
      </Text>
      {ticket ? <Text style={styles.ticket}>{ticket}</Text> : null}
      {onFold ? (
        <Pressable
          onPress={onFold}
          hitSlop={10}
          style={[styles.fold, !ticket && { marginLeft: 'auto' }]}
          accessibilityRole="button"
          accessibilityLabel="Fold the questions away">
          <View style={styles.down}>
            <Icon name="IconChevron" size={10} color={colors.inkFaint} />
          </View>
        </Pressable>
      ) : null}
    </View>
  );
}

/** A question as it reads once answered: its words, and the answer in one line, or only that it was answered when the answer is a secret. */
export function AnsweredQuestion({
  own,
  asker,
  cli,
  question,
  answer,
  secret,
}: {
  /** This chat's own agent asked, so who asked goes without saying. */
  own: boolean;
  asker: string;
  cli: string;
  question: string;
  answer: string;
  secret: boolean;
}) {
  const styles = useStyles(makeStyles);
  return (
    <View>
      {own ? null : (
        <AskedBy cli={cli}>
          <Text style={styles.name}>{asker}</Text> asked you
        </AskedBy>
      )}
      <Text style={styles.prose} selectable>
        {question}
      </Text>
      {secret ? (
        <Private text={SECRET_ANSWERED} />
      ) : (
        <Text style={styles.answered} selectable>
          You answered: <Text style={styles.answer}>{answer}</Text>
        </Text>
      )}
    </View>
  );
}

/** How much a hidden answer holds, so the person can see their typing and paste landed. */
function hiddenSize(text: string) {
  const lines = text.replace(/\n$/, '').split('\n').length;
  const characters = `${text.length} character${text.length === 1 ? '' : 's'} hidden`;
  return lines > 1 ? `${lines} lines, ${characters}` : characters;
}

/**
 * The answer to a secret question, typed or pasted into a field that keeps nothing once it is held.
 * A secure field cannot hold line breaks, so this one draws its text invisibly.
 */
function SecretAnswer({ question, onKeep }: { question: AgentQuestion; onKeep: (text: string) => Promise<boolean> }) {
  const styles = useStyles(makeStyles);
  const [text, setText] = useState('');
  const keep = () => {
    if (text) void onKeep(text).then((kept) => kept && setText(''));
  };
  return (
    <View style={styles.secret}>
      <View style={styles.secretField}>
        <Field
          value={text}
          onChangeText={setText}
          multiline
          autoComplete="off"
          autoCorrect={false}
          autoCapitalize="none"
          spellCheck={false}
          textContentType="none"
          importantForAutofill="no"
          placeholder="Type or paste the answer"
          accessibilityLabel={`Private answer to ${question.name}`}
          style={styles.secretInput}
        />
        <Chip title="Send" primary onPress={keep} />
      </View>
      {text ? <Text style={styles.secretSize}>{hiddenSize(text)}</Text> : null}
      <Private text={question.secretNote ?? ''} />
    </View>
  );
}

function Private({ text }: { text: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.private}>
      <Icon name="IconLock" size={12} color={colors.inkFaint} />
      <Text style={styles.privateText}>{text}</Text>
    </View>
  );
}

/** A question's name on its tab: the words the agent gave it, else its place among the rest. */
function title(question: AgentQuestion, index: number, count: number) {
  return question.header ?? (count > 1 ? `Question ${index + 1}` : 'Question');
}

function Key({ multi, on }: { multi: boolean; on: boolean }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={[styles.mark, multi ? styles.tick : styles.radio, on && styles.markOn, on && multi && { backgroundColor: colors.ink }]}>
      {on ? multi ? <Icon name="IconCheck" size={10} color={colors.ground} /> : <View style={styles.dot} /> : null}
    </View>
  );
}

function Tabs({
  questions,
  current,
  onShow,
}: {
  questions: ChatQuestions;
  current: AgentQuestion;
  onShow: (question: AgentQuestion) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const count = questions.open.length;
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} style={styles.tabs} contentContainerStyle={styles.tabsRow}>
      {questions.open.map((question, index) => {
        const on = question.id === current.id;
        return (
          <Pressable
            key={question.id}
            onPress={() => {
              haptics.select();
              onShow(question);
            }}
            style={[styles.tab, on && styles.tabOn]}
            accessibilityRole="tab"
            accessibilityState={{ selected: on }}
            accessibilityLabel={`${title(question, index, count)}${questions.picks.has(question.id) ? ', answered' : ''}`}>
            {questions.picks.has(question.id) ? (
              <Icon name="IconCheck" size={11} color={colors.inkFaint} />
            ) : (
              <Text style={styles.tabNumber}>{index + 1}</Text>
            )}
            <Text style={[styles.tabText, on && { color: colors.ink }]} numberOfLines={1}>
              {title(question, index, count)}
            </Text>
          </Pressable>
        );
      })}
    </ScrollView>
  );
}

function Folded({ open, onOpen }: { open: AgentQuestion[]; onOpen: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const names = open.map((question, index) => title(question, index, open.length)).join(' · ');
  return (
    <Pressable
      onPress={() => {
        haptics.select();
        onOpen();
      }}
      style={styles.folded}
      accessibilityRole="button"
      accessibilityLabel={`${open.length} ${open.length === 1 ? 'question' : 'questions'}: ${names}. Open them`}>
      <Text style={styles.foldedLabel}>{open.length === 1 ? 'QUESTION' : 'QUESTIONS'}</Text>
      <Text style={styles.foldedCount}>{open.length}</Text>
      <Text style={styles.foldedNames} numberOfLines={1}>
        {names}
      </Text>
      <Icon name="IconChevron" size={10} color={colors.inkFaint} />
    </Pressable>
  );
}

/**
 * The questions an agent put to the person and waits on, at the end of its chat: a tab for each when there are
 * several, the words of the one on show as prose, then its answers as rows with a ring, or a tick on a question that
 * takes several. Picks are held until each question has one and go together with Send; a lone question that takes one
 * answer goes on the tap. "Something else…" hands over to the composer; a secret is typed into its own masked field
 * instead, so it never reaches the composer's draft. A watching phone shows the questions without the rows.
 */
export function QuestionDock({
  questions,
  canAnswer,
  onWrite,
}: {
  questions: ChatQuestions;
  canAnswer: boolean;
  onWrite: (question: AgentQuestion) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const { open, current, picks } = questions;
  if (!current) return null;
  if (questions.folded) return <Folded open={open} onOpen={() => questions.fold(false)} />;
  const picked = picks.get(current.id);
  const given = picked?.given;
  const chosen = (index: number) =>
    given !== undefined && (('choice' in given && given.choice === index) || ('choices' in given && given.choices.includes(index)));
  const written = given && 'answer' in given ? picked : undefined;
  const writing = questions.writing && questions.composing?.id === current.id;
  const via = current.viaNote;
  const row = (key: string | number, mark: ReactNode, on: boolean, label: ReactNode, a11y: string, onPress: () => void) => (
    <Pressable
      key={key}
      onPress={() => {
        haptics.select();
        onPress();
      }}
      style={({ pressed }) => [styles.option, (on || pressed) && { backgroundColor: colors.active }]}
      accessibilityRole={current.multi ? 'checkbox' : 'radio'}
      accessibilityState={{ checked: on }}
      accessibilityLabel={a11y}>
      {mark}
      <Text style={styles.label}>{label}</Text>
    </Pressable>
  );
  const pencil = (
    <View style={styles.pencil}>
      <Icon name="IconPencil" size={12} color={colors.inkFaint} />
    </View>
  );
  const other = written
    ? row('other', pencil, true, written.shown, `Your answer, ${written.shown}`, () => onWrite(current))
    : row(
        'other',
        pencil,
        writing,
        <Text style={{ color: colors.inkDim }}>
          Something else…{writing ? <Text style={styles.why}>{'   answering below'}</Text> : null}
        </Text>,
        'Something else',
        () => onWrite(current),
      );
  const sends = open.length > 1 || current.multi || written !== undefined;
  return (
    <View accessibilityLiveRegion="polite">
      <AskedBy cli={current.cli} ticket={current.ticket} onFold={() => questions.fold(true)}>
        <Text style={styles.name}>{current.name}</Text> asks you{via ? ` · ${via}` : ''}
      </AskedBy>
      {open.length > 1 ? <Tabs questions={questions} current={current} onShow={questions.show} /> : null}
      <Text style={styles.prose} selectable>
        {current.text}
        {current.multi ? <Text style={styles.pickAny}> Pick any.</Text> : null}
      </Text>
      {canAnswer && (current.choices.length > 0 || !current.secret) ? (
        <View style={styles.options}>
          {current.choices.map((choice, index) =>
            row(
              index,
              <Key multi={current.multi} on={chosen(index)} />,
              chosen(index),
              <>
                {choice.label}
                {choice.recommended ? <Text style={styles.recommended}>{'  recommended'}</Text> : null}
                {choice.detail ? <Text style={styles.why}>{`   ${choice.detail}`}</Text> : null}
              </>,
              [choice.label, choice.recommended && 'recommended', choice.detail].filter(Boolean).join(', '),
              () => questions.choose(current, index),
            ),
          )}
          {current.secret ? null : other}
        </View>
      ) : null}
      {canAnswer && current.secret ? (
        picked ? (
          <Private text="Answer kept, sent with the rest" />
        ) : (
          <SecretAnswer question={current} onKeep={(text) => questions.hold(current, text)} />
        )
      ) : null}
      {canAnswer && sends ? (
        <View style={styles.send}>
          <Chip title="Send" primary={questions.ready} disabled={!questions.ready} onPress={questions.sendAll} />
        </View>
      ) : null}
    </View>
  );
}

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
        <Text style={styles.name}>{question.name}</Text> assumed {question.assumption}
        {canAnswer ? (
          <>
            {'  ·  '}
            {act('Fine', onFine)}
            {'   '}
            {act('Change', onChange)}
          </>
        ) : null}
      </Text>
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
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
    answered: { marginTop: 4, marginBottom: 8, fontFamily: fonts.ui, fontSize: 13, lineHeight: 19, color: colors.inkDim },
    answer: { color: colors.ink },
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
