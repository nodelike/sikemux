import { memo, useEffect, useState } from 'react';
import { Image, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import type { ConnectionLike } from '@sikemux/native';

import { formatAge } from '@shared/lib/format';
import { useExternalValue } from '@shared/lib/externalValue';
import {
  ADD_NOTE,
  AGAINST_IT,
  CONNECTING,
  context as contextOf,
  goesOn,
  GO_WITH_THIS,
  hidden,
  kept,
  NOT_MINE,
  OWN_WORDS,
  PICK,
  pictureLabel,
  place,
  pushFailed,
  SECRET_FIELD,
  SHOW_ALL,
  SHOW_LESS,
  WHAT_RUNS,
} from '@shared/calls/words';
import { useT } from '@/i18n';
import type { CallOption, PersonCall } from '@shared/wire.generated';
import { AskerMark } from '@/calls/AskerMark';
import { SECRET_INPUT } from '@/calls/secretInput';
import { answer, answerOf, choose, chosenIn, draftOf, edit, focus, goesOnAsIs, notMine, see, useKept } from '@/calls/hostCalls';
import { useHostFile } from '@/chat/hostFiles';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { useMinute } from '@/ui/motion';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

const ROW = 44;

/**
 * A call docked on its asker's chat, above the composer: the Mac's call card at the phone's sizes, a little tighter,
 * with 44-point rows and Go with this across the card. Typing in the composer answers it too, so it has no Open chat.
 * When the chat waits on several calls, the card says which it is and the others wait folded under it.
 */
export const CallDock = memo(function CallDock({
  core,
  calls,
  call,
  chat,
  queue,
  connection,
  canAnswer,
  report,
  inPlace = false,
}: {
  core: string;
  /** The host's calls, which the hub keeps as the same array while none changes. */
  calls: readonly PersonCall[] | undefined;
  call: PersonCall;
  /** The agent whose chat the card docks on. */
  chat: string;
  /** Every call waiting in that chat, oldest first, `call` among them. */
  queue: PersonCall[];
  connection: ConnectionLike | undefined;
  canAnswer: boolean;
  report: (text: string) => void;
  /** Unfolded under its row in a list that scrolls, so the card takes its full height rather than scrolling itself. */
  inPlace?: boolean;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const minute = useMinute();
  const held = useKept(core);
  const t = useT();
  const store = draftOf(calls, call);
  const draft = useExternalValue(store);
  const chosen = chosenIn(draft, call);
  const typed = draft.typed.trim().length > 0;
  const [focused, setFocused] = useState(false);
  const lit = focused || typed;
  const secret = !!call.secret;
  const pictured = !secret && call.options.some((option) => option.picture);
  const runs = call.kind === 'production' ? call.action : undefined;
  const at = queue.findIndex((each) => each.id === call.id);
  const context = contextOf(call);
  // What a step to production, or anything that can't be undone, rests on is read whole; any other call's opens on a tap.
  const whole = call.kind === 'production' || !!call.brief?.cannotUndo;
  const [contextOpen, setContextOpen] = useState(false);
  const failed = held.failed[call.id];
  // The core asks only a production call to have been drawn before its answer counts.
  const seen = call.kind !== 'production' || held.seen.has(call.id);
  const ready = (chosen.length > 0 || typed || goesOnAsIs(call)) && !draft.sending && canAnswer && seen;

  // Tells the host this phone drew the call, again whenever the connection changes.
  useEffect(() => {
    if (connection) void see(core, () => connection, call);
  }, [core, connection, call]);

  const go = () => {
    const params = answerOf(store.get(), call);
    if (!params || !ready) return;
    haptics.select();
    void answer(core, connection, call, store, params, report);
  };

  return (
    <View style={[styles.card, inPlace && styles.cardInPlace]}>
      <ScrollView style={styles.scroll} contentContainerStyle={styles.inner} scrollEnabled={!inPlace} persistentScrollbar>
        <View style={styles.who}>
          <AskerMark agentId={call.asker.agentId} provider={call.asker.cli} chief={call.asker.chief} asking />
          <Text style={styles.name}>@{call.asker.name}</Text>
          <Text style={styles.sep}>·</Text>
          <Text style={styles.age}>{formatAge(call.raisedAt, minute * 60_000, t)}</Text>
          {queue.length > 1 && at >= 0 ? <Text style={styles.place}>{place(at + 1, queue.length, t)}</Text> : null}
        </View>
        <Text style={styles.question}>{call.question}</Text>
        {context ? (
          <Text
            style={styles.context}
            numberOfLines={whole || contextOpen ? undefined : 2}
            onPress={whole ? undefined : () => setContextOpen(!contextOpen)}
            accessibilityRole={whole ? undefined : 'button'}
            accessibilityState={whole ? undefined : { expanded: contextOpen }}
            accessibilityHint={whole ? undefined : t(contextOpen ? SHOW_LESS : SHOW_ALL)}>
            {context}
          </Text>
        ) : null}
        {(call.evidence ?? [])
          .filter((evidence) => !evidence.picture)
          .map((evidence, index) => (
            <Text key={index} style={styles.context}>
              {evidence.what}
            </Text>
          ))}
        {pictured ? <Strip call={call} chosen={chosen} onChoose={(option) => choose(store, call, option)} /> : <Evidence call={call} />}
        <View style={[styles.options, { borderColor: lit ? colors.borderSelected : colors.border }]}>
          {call.options.map((option, index) => (
            <OptionRow
              key={option.id}
              option={option}
              first={index === 0}
              on={chosen.includes(option.id)}
              multi={!!call.multi}
              pick={call.pick === option.id}
              disabled={!canAnswer || draft.sending}
              onPress={() => {
                haptics.select();
                choose(store, call, option.id);
              }}
            />
          ))}
          <View style={[styles.field, call.options.length > 0 && styles.fieldAfter]}>
            <Icon name="IconPencil" size={13} color={lit ? colors.tertiary : colors.inkFaint} />
            {secret ? (
              <TextInput
                value={draft.typed}
                onChangeText={(text) => edit(store, { typed: text })}
                placeholder={t(SECRET_FIELD)}
                placeholderTextColor={colors.tertiary}
                // Its words never show, and the keyboard neither suggests nor remembers them. A hidden secure field
                // cannot hold the line breaks of a pasted key, so the text is drawn clear; paste still works.
                style={[styles.input, draft.typed ? styles.hiddenText : null]}
                multiline
                {...SECRET_INPUT}
                caretHidden={!!draft.typed}
                editable={canAnswer}
                onFocus={() => setFocused(true)}
                onBlur={() => setFocused(false)}
              />
            ) : (
              <TextInput
                value={draft.typed}
                onChangeText={(text) => edit(store, { typed: text })}
                placeholder={chosen.length ? t(ADD_NOTE) : t(OWN_WORDS)}
                placeholderTextColor={colors.tertiary}
                style={styles.input}
                // Only the button passes a step to production; Enter there is a new line of the note's.
                returnKeyType={call.kind === 'production' ? 'default' : 'send'}
                onSubmitEditing={call.kind === 'production' ? undefined : go}
                editable={canAnswer}
                onFocus={() => setFocused(true)}
                onBlur={() => setFocused(false)}
              />
            )}
          </View>
        </View>
        {call.against ? (
          <Text style={styles.against}>
            <Text style={{ color: colors.tertiary }}>{t(AGAINST_IT)} </Text>
            {call.against}
          </Text>
        ) : null}
        {runs || (call.canHandBack && canAnswer) ? (
          <View style={styles.extras}>
            {runs ? (
              <Pressable
                onPress={() => edit(store, { runsOpen: !draft.runsOpen })}
                style={styles.runs}
                accessibilityRole="button"
                accessibilityState={{ expanded: draft.runsOpen }}>
                <View style={{ transform: [{ rotate: draft.runsOpen ? '90deg' : '0deg' }] }}>
                  <Icon name="IconChevron" size={11} color={colors.tertiary} />
                </View>
                <Text style={styles.extraText}>{t(WHAT_RUNS)}</Text>
              </Pressable>
            ) : null}
            <View style={{ flex: 1 }} />
            {call.canHandBack && canAnswer ? (
              <Pressable
                disabled={draft.sending}
                onPress={() => {
                  haptics.select();
                  void notMine(connection, call, store, report);
                }}
                style={styles.runs}
                accessibilityRole="button">
                <Text style={styles.notMine}>{t(NOT_MINE)}</Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}
        {runs && draft.runsOpen ? (
          <Text style={styles.command} selectable>
            {runs}
          </Text>
        ) : null}
        {secret ? (
          <>
            {typed ? <Text style={styles.hiddenSize}>{hidden(draft.typed, t)}</Text> : null}
            <Text style={styles.kept}>{kept(call.asker.name, t)}</Text>
          </>
        ) : null}
        {failed ? <Text style={styles.failed}>{pushFailed(failed, t)}</Text> : null}
        {call.assumption ? (
          <View style={styles.goesOn}>
            <Icon name="IconClock" size={13} color={colors.tertiary} />
            <Text style={styles.goesOnText}>{goesOn(call.asker.name, call.assumption, t)}</Text>
          </View>
        ) : null}
        <Pressable
          onPress={go}
          disabled={!ready}
          style={({ pressed }) => [styles.go, !ready && { opacity: 0.5 }, pressed && { opacity: 0.8 }]}
          accessibilityRole="button"
          accessibilityState={{ disabled: !ready }}>
          <Text style={styles.goText}>{seen ? t(GO_WITH_THIS) : t(CONNECTING)}</Text>
        </Pressable>
        {queue
          .filter((other) => other.id !== call.id)
          .map((other) => (
            <Pressable
              key={other.id}
              onPress={() => {
                haptics.select();
                focus(core, other, chat);
              }}
              style={styles.folded}
              accessibilityRole="button">
              <AskerMark agentId={other.asker.agentId} provider={other.asker.cli} chief={other.asker.chief} asking size={16} />
              <Text style={styles.foldedText} numberOfLines={1}>
                {other.question}
              </Text>
              <Text style={styles.foldedAge}>{formatAge(other.raisedAt, minute * 60_000, t)}</Text>
            </Pressable>
          ))}
      </ScrollView>
    </View>
  );
});

function OptionRow({
  option,
  first,
  on,
  multi,
  pick,
  disabled,
  onPress,
}: {
  option: CallOption;
  first: boolean;
  on: boolean;
  multi: boolean;
  pick: boolean;
  disabled: boolean;
  onPress: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      style={[styles.option, !first && styles.optionAfter, on && { backgroundColor: colors.active }]}
      accessibilityRole={multi ? 'checkbox' : 'radio'}
      accessibilityState={{ checked: on, disabled }}>
      <ChoiceMark on={on} multi={multi} />
      <View style={styles.optionWords}>
        <Text style={styles.optionLabel}>{option.label}</Text>
        {option.detail ? <Text style={styles.optionDetail}>{option.detail}</Text> : null}
      </View>
      {pick ? (
        <View style={styles.pick}>
          <Icon name="Logo" size={13} color={colors.accent} />
          <Text style={styles.pickText}>{t(PICK)}</Text>
        </View>
      ) : null}
    </Pressable>
  );
}

/** The ring of an option on a call that takes one, or the box of one on a call that takes several. */
function ChoiceMark({ on, multi }: { on: boolean; multi: boolean }) {
  const colors = useColors();
  if (multi) {
    return (
      <View
        style={{
          width: 18,
          height: 18,
          borderRadius: 4,
          borderWidth: 1.5,
          borderColor: on ? colors.accent : colors.inkFaint,
          backgroundColor: on ? colors.accent : 'transparent',
          alignItems: 'center',
          justifyContent: 'center',
        }}>
        {on ? <Icon name="IconCheck" size={11} color={colors.ground} /> : null}
      </View>
    );
  }
  return (
    <View
      style={{
        width: 18,
        height: 18,
        borderRadius: 9,
        borderWidth: on ? 5 : 1.5,
        borderColor: on ? colors.accent : colors.inkFaint,
        backgroundColor: on ? '#fff' : 'transparent',
      }}
    />
  );
}

/** A taste call's pictures: the chosen option's leads, large, the others smaller and dimmed beside it. */
function Strip({ call, chosen, onChoose }: { call: PersonCall; chosen: string[]; onChoose: (option: string) => void }) {
  const styles = useStyles(makeStyles);
  const lead = Math.max(
    0,
    call.options.findIndex((option) => option.id === (chosen[0] ?? call.pick)),
  );
  return (
    <View style={styles.strip}>
      <CallPicture path={call.options[lead].picture} style={styles.lead} />
      <View style={styles.rest}>
        {call.options.map((option, index) =>
          index === lead ? null : (
            <Pressable
              key={option.id}
              style={styles.restItem}
              onPress={() => onChoose(option.id)}
              accessibilityRole="button"
              accessibilityLabel={option.label}>
              <CallPicture path={option.picture} style={[styles.fill, { opacity: 0.72 }]} />
            </Pressable>
          ),
        )}
      </View>
    </View>
  );
}

/** A call's evidence pictures from their top, two side by side, each with its own short words as its name. */
function Evidence({ call }: { call: PersonCall }) {
  const styles = useStyles(makeStyles);
  const shown = (call.evidence ?? []).filter((evidence) => evidence.picture).slice(0, 2);
  if (!shown.length) return null;
  return (
    <View style={styles.evidence}>
      {shown.map((evidence, index) => {
        const label = pictureLabel(evidence.what);
        return (
          <View key={index} style={styles.evidenceItem} accessible accessibilityLabel={evidence.what}>
            <CallPicture path={evidence.picture} style={styles.fill} top />
            {label ? (
              <View style={styles.label}>
                <Text style={styles.labelText} numberOfLines={1}>
                  {label}
                </Text>
              </View>
            ) : null}
          </View>
        );
      })}
    </View>
  );
}

/** A picture a call names on the host, fetched to this phone; evidence shows a page from its top. */
function CallPicture({ path, style, top }: { path?: string; style: object; top?: boolean }) {
  const styles = useStyles(makeStyles);
  const uri = useHostFile(path ?? '', !!path, 1200);
  return (
    <View style={[styles.picture, style]}>
      {uri ? (
        <Image
          source={{ uri }}
          style={top ? styles.topImage : StyleSheet.absoluteFill}
          resizeMode="cover"
          accessibilityIgnoresInvertColors
        />
      ) : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    card: {
      marginBottom: 8,
      borderRadius: 16,
      borderWidth: 1,
      borderColor: colors.borderStrong,
      backgroundColor: colors.overlay,
      maxHeight: 520,
      overflow: 'hidden',
    },
    cardInPlace: { maxHeight: undefined },
    scroll: { flexGrow: 0 },
    inner: { padding: 12 },
    who: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    name: { fontFamily: fonts.uiMedium, fontSize: 14, color: colors.secondary },
    sep: { color: colors.inkFaint },
    age: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary },
    place: { marginLeft: 'auto', fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary, fontVariant: ['tabular-nums'] },
    question: { marginTop: 8, fontFamily: fonts.uiSemibold, fontSize: 17, lineHeight: 22, color: colors.ink },
    context: { marginTop: 2, fontFamily: fonts.ui, fontSize: 14, lineHeight: 19, color: colors.tertiary },
    options: { marginTop: 12, borderWidth: 1, borderRadius: 10, overflow: 'hidden' },
    option: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: ROW, paddingHorizontal: 14, paddingVertical: 8 },
    optionAfter: { borderTopWidth: 1, borderTopColor: colors.border },
    optionWords: { flex: 1, gap: 1 },
    optionLabel: { fontFamily: fonts.ui, fontSize: 15.5, color: colors.ink },
    optionDetail: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    pick: { flexDirection: 'row', alignItems: 'center', gap: 5 },
    pickText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    field: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: ROW, paddingHorizontal: 14, backgroundColor: colors.sunken },
    fieldAfter: { borderTopWidth: 1, borderTopColor: colors.border },
    input: { flex: 1, fontFamily: fonts.ui, fontSize: 15.5, color: colors.ink, paddingVertical: 10 },
    hiddenText: { color: 'transparent' },
    against: { marginTop: 10, fontFamily: fonts.ui, fontSize: 13.5, color: colors.secondary },
    extras: { marginTop: 6, flexDirection: 'row', alignItems: 'center', minHeight: 32 },
    runs: { flexDirection: 'row', alignItems: 'center', gap: 6, minHeight: 32 },
    extraText: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    notMine: { fontFamily: fonts.uiMedium, fontSize: 13, color: colors.secondary },
    command: { fontFamily: fonts.mono, fontSize: 12.5, color: colors.secondary },
    hiddenSize: { marginTop: 8, fontFamily: fonts.ui, fontSize: 12.5, color: colors.inkDim },
    kept: { marginTop: 4, fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    failed: { marginTop: 10, fontFamily: fonts.ui, fontSize: 13, color: colors.danger },
    goesOn: { marginTop: 12, flexDirection: 'row', alignItems: 'flex-start', gap: 6 },
    goesOnText: { flex: 1, fontFamily: fonts.ui, fontSize: 13.5, lineHeight: 19, color: colors.tertiary },
    go: { marginTop: 12, height: 46, borderRadius: 12, backgroundColor: colors.accent, alignItems: 'center', justifyContent: 'center' },
    goText: { fontFamily: fonts.uiSemibold, fontSize: 16, color: colors.ground },
    folded: { flexDirection: 'row', alignItems: 'center', gap: 10, minHeight: ROW, marginTop: 4 },
    foldedText: { flex: 1, fontFamily: fonts.ui, fontSize: 14, color: colors.secondary },
    foldedAge: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.warn },
    strip: { marginTop: 10, height: 120, flexDirection: 'row', gap: 6 },
    lead: { flex: 2 },
    rest: { width: 104, gap: 6 },
    restItem: { flex: 1 },
    fill: { flex: 1 },
    evidence: { marginTop: 10, height: 128, flexDirection: 'row', gap: 6 },
    evidenceItem: { flex: 1 },
    picture: { borderRadius: 10, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.sunken, overflow: 'hidden' },
    topImage: { position: 'absolute', top: 0, left: 0, width: '100%', aspectRatio: 0.75 },
    label: {
      position: 'absolute',
      left: 6,
      bottom: 6,
      maxWidth: '90%',
      paddingHorizontal: 7,
      paddingVertical: 3,
      borderRadius: 6,
      backgroundColor: 'rgba(0,0,0,0.62)',
    },
    labelText: { fontFamily: fonts.uiMedium, fontSize: 12, color: '#fff' },
  });
};
