import { useState, type ReactNode } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { SECRET_ANSWERED } from '@mac/chat/agentLetters';
import type { AgentQuestion } from '@/core/protocol';
import { Chip, Field } from '@/ui/controls';
import { haptics } from '@/ui/haptics';
import { AgentIcon, Icon } from '@/ui/Icon';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** Who is asking, above the question itself, and the ticket of the board card it works. */
function AskedBy({ cli, ticket, children }: { cli: string; ticket?: string; children: ReactNode }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.who}>
      <AgentIcon provider={cli} size={14} />
      <Text style={styles.whoText} numberOfLines={1}>
        {children}
      </Text>
      {ticket ? <Text style={styles.ticket}>{ticket}</Text> : null}
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
  const colors = useColors();
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
        <View style={styles.private}>
          <Icon name="IconLock" size={12} color={colors.inkFaint} />
          <Text style={styles.privateText}>{SECRET_ANSWERED}</Text>
        </View>
      ) : (
        <Text style={styles.answered} selectable>
          You answered: <Text style={styles.answer}>{answer}</Text>
        </Text>
      )}
    </View>
  );
}

/** The answer to a secret question, typed into a masked field that keeps nothing once it is sent. */
function SecretAnswer({ question, onSend }: { question: AgentQuestion; onSend: (text: string) => Promise<boolean> }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const [text, setText] = useState('');
  const send = () => {
    if (text) void onSend(text).then((sent) => sent && setText(''));
  };
  return (
    <View style={styles.secret}>
      <View style={styles.secretField}>
        <Field
          value={text}
          onChangeText={setText}
          onSubmitEditing={send}
          secureTextEntry
          autoComplete="off"
          autoCorrect={false}
          autoCapitalize="none"
          spellCheck={false}
          textContentType="none"
          importantForAutofill="no"
          returnKeyType="send"
          placeholder="Type the answer"
          accessibilityLabel={`Private answer to ${question.name}`}
          style={styles.secretInput}
        />
        <Chip title="Send" primary onPress={send} />
      </View>
      <View style={styles.private}>
        <Icon name="IconLock" size={12} color={colors.inkFaint} />
        <Text style={styles.privateText}>{question.secretNote}</Text>
      </View>
    </View>
  );
}

/**
 * A question an agent put to the person, at the end of its chat: its words as prose, then the answers it offered as
 * numbered rows. "Something else…" hands over to the composer; a secret is typed into its own masked field instead, so
 * it never reaches the composer's draft. A watching phone shows the question without the rows.
 */
export function OpenQuestion({
  question,
  canAnswer,
  writing,
  onChoose,
  onWrite,
  onSecret,
}: {
  question: AgentQuestion;
  canAnswer: boolean;
  writing: boolean;
  onChoose: (choice: number) => void;
  onWrite: () => void;
  onSecret: (text: string) => Promise<boolean>;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const via = question.viaNote;
  const row = (key: string | number, number: number, label: string, detail: string | undefined, other: boolean, onPress: () => void) => (
    <Pressable
      key={key}
      onPress={() => {
        haptics.select();
        onPress();
      }}
      style={({ pressed }) => [styles.option, (pressed || (other && writing)) && { backgroundColor: colors.active }]}
      accessibilityRole="button"
      accessibilityLabel={detail ? `${label}, ${detail}` : label}>
      <Text style={styles.key}>{number}</Text>
      <Text style={[styles.label, other && { color: colors.inkDim }]}>
        {label}
        {detail ? <Text style={styles.why}>{`   ${detail}`}</Text> : null}
      </Text>
    </Pressable>
  );
  return (
    <View accessibilityLiveRegion="polite">
      <AskedBy cli={question.cli} ticket={question.ticket}>
        <Text style={styles.name}>{question.name}</Text> asks you{via ? ` · ${via}` : ''}
      </AskedBy>
      <Text style={styles.prose} selectable>
        {question.text}
      </Text>
      {canAnswer && (question.choices.length > 0 || !question.secret) ? (
        <View style={styles.options}>
          {question.choices.map((choice, index) => row(index, index + 1, choice.label, choice.detail, false, () => onChoose(index)))}
          {question.secret
            ? null
            : row('other', question.choices.length + 1, 'Something else…', writing ? 'answering below' : undefined, true, onWrite)}
        </View>
      ) : null}
      {canAnswer && question.secret ? <SecretAnswer question={question} onSend={onSecret} /> : null}
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
    options: { marginTop: 6, marginBottom: 4, marginHorizontal: -10 },
    option: { flexDirection: 'row', alignItems: 'baseline', gap: 12, minHeight: 40, padding: 10, borderRadius: 10 },
    key: { width: 10, fontFamily: fonts.mono, fontSize: 12, color: colors.inkFaint },
    label: { flex: 1, fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 20, color: colors.ink },
    why: { fontSize: 13, color: colors.inkDim },
    answered: { marginTop: 4, marginBottom: 8, fontFamily: fonts.ui, fontSize: 13, lineHeight: 19, color: colors.inkDim },
    answer: { color: colors.ink },
    ticket: { fontFamily: fonts.mono, fontSize: 11, color: colors.inkDim, marginLeft: 'auto' },
    assumed: { flexDirection: 'row', alignItems: 'flex-start', gap: 7, marginTop: 10, marginBottom: 6 },
    check: { height: 18, justifyContent: 'center' },
    assumedText: { flex: 1, fontFamily: fonts.ui, fontSize: 12.5, lineHeight: 18, color: colors.inkDim },
    act: { fontFamily: fonts.uiMedium, color: colors.ink },
    secret: { marginTop: 8, marginBottom: 6, gap: 8 },
    secretField: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    secretInput: { flex: 1 },
    private: { flexDirection: 'row', alignItems: 'flex-start', gap: 6, marginTop: 4, marginBottom: 8 },
    privateText: { flex: 1, fontFamily: fonts.ui, fontSize: 12.5, lineHeight: 18, color: colors.inkDim },
  });
