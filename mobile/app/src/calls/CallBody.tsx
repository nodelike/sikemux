import { memo, type ReactNode, useEffect, useState } from 'react';
import { Image, Pressable, StyleSheet, Text, TextInput, View } from 'react-native';
import { router } from 'expo-router';
import type { ConnectionLike } from '@sikemux/native';

import { formatAge } from '@shared/lib/format';
import { useExternalValue } from '@shared/lib/externalValue';
import {
  AGAINST_IT,
  CONNECTING,
  context as contextOf,
  DO_NOT_DOING,
  DONE,
  GO_WITH_THIS,
  goWith,
  headline as headlineOf,
  kept,
  LOOK_SEEN,
  NEXT_CALL,
  OPEN_CHAT,
  PICK,
  pictureLabel,
  pushFailed,
  SEEN,
  WHAT_RUNS,
  WHY_IT_CAME_UP,
  WHY_NOT_DOING,
} from '@shared/calls/words';
import { useT } from '@/i18n';
import type { CallAsker, CallLink, CallOption, PersonCall } from '@shared/wire.generated';
import { PageCard } from '@/chat/Notices';
import { serviceMark } from '@/chat/Markdown';
import { answer, answerOf, choose, chosenIn, draftOf, edit, see, useKept, type Draft } from '@/calls/hostCalls';
import { HostContext, useHostFile } from '@/chat/hostFiles';
import { AgentPill, ChiefMark, namedAgent, type Named, openChat, useCrew } from '@/devices/Crew';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { SectionLabel } from '@/ui/list';
import { useMinute } from '@/ui/motion';
import { openLink } from '@/ui/prompt';
import { fonts, type Palette, useColors, useStyles } from '@/ui/theme';

/** Whether a call's answer can go: something chosen or typed, as its kind asks, while nothing is on its way. */
export function readyOf(call: PersonCall, draft: Draft, canAnswer: boolean, seen: boolean): boolean {
  const chosen = chosenIn(draft, call);
  const typed = draft.typed.trim().length > 0;
  const answered =
    call.kind === 'look'
      ? true
      : call.kind === 'do'
        ? chosen.length > 0 && (!chosen.includes(DO_NOT_DOING) || typed)
        : chosen.length > 0 || typed;
  return answered && !draft.sending && canAnswer && seen;
}

/**
 * A call drawn as its own page, as the Mac draws a call's tab: who asked, the question, the choices as full-width
 * tiles, Go, and why it came up. The words that go with the answer are typed in the composer under the page.
 */
export const CallBody = memo(function CallBody({
  core,
  calls,
  call,
  next,
  connection,
  canAnswer,
  report,
}: {
  core: string;
  /** The host's calls, which the hub keeps as the same array while none changes. */
  calls: readonly PersonCall[] | undefined;
  call: PersonCall;
  next: PersonCall | undefined;
  connection: ConnectionLike | undefined;
  canAnswer: boolean;
  report: (text: string) => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const minute = useMinute();
  const held = useKept(core);
  const t = useT();
  const store = draftOf(calls, call);
  const draft = useExternalValue(store);
  const chosen = chosenIn(draft, call);
  const secret = !!call.secret;
  const pictured = !secret && call.options.some((option) => option.picture);
  const runs = call.kind === 'production' ? call.action : undefined;
  const failed = held.failed[call.id];
  // The core asks only a production call to have been drawn before its answer counts.
  const seen = call.kind !== 'production' || held.seen.has(call.id);
  const notDoing = call.kind === 'do' && chosen.includes(DO_NOT_DOING);
  const ready = readyOf(call, draft, canAnswer, seen);
  const headline = headlineOf(call, t);
  const chosenLabel = call.options.find((option) => option.id === chosen[0])?.label;
  const hasGo = call.kind === 'look' || call.kind === 'do' || call.options.length > 0;
  const goWords =
    call.kind === 'look'
      ? t(SEEN)
      : call.kind === 'do'
        ? (chosenLabel ?? t(DONE))
        : !seen
          ? t(CONNECTING)
          : chosenLabel
            ? goWith(chosenLabel, t)
            : t(GO_WITH_THIS);

  // Tells the host this phone drew the call, again whenever the connection changes.
  useEffect(() => {
    if (connection) void see(core, () => connection, call);
  }, [core, connection, call]);

  const go = () => {
    const params = call.kind === 'look' ? { id: call.id, option: LOOK_SEEN } : answerOf(store.get(), call);
    if (!params || !ready) return;
    haptics.select();
    void answer(core, connection, call, store, params, report);
  };

  return (
    <View>
      <View style={styles.who}>
        <HostContext value={core}>
          <Asker asker={call.asker} />
          <Text style={styles.sep}>·</Text>
          <Text style={styles.age}>{formatAge(call.raisedAt, minute * 60_000, t)}</Text>
          {call.about ? (
            <>
              <Text style={styles.sep}>·</Text>
              <Text style={styles.age}>{t('about')}</Text>
              <Asker asker={call.about} />
            </>
          ) : null}
        </HostContext>
        <View style={{ flex: 1 }} />
        <Pressable onPress={() => openChat(core, call.asker.agentId)} hitSlop={8} accessibilityRole="button">
          <Text style={styles.openChat}>{t(OPEN_CHAT)}</Text>
        </Pressable>
      </View>
      <Text style={styles.question} selectable>
        {headline}
      </Text>
      {headline !== call.question ? (
        <Text style={styles.prose} selectable>
          {call.question}
        </Text>
      ) : null}
      {call.link ? (
        <LinkTile
          link={call.link}
          onOpen={() => {
            if (call.kind === 'look' && canAnswer && !draft.sending) {
              haptics.select();
              void answer(core, connection, call, store, { id: call.id, option: LOOK_SEEN }, report);
            }
          }}
        />
      ) : null}
      {call.page ? <PageCard page={call.page} /> : null}
      {pictured ? <Strip call={call} chosen={chosen} onChoose={(option) => choose(store, call, option)} /> : <Evidence call={call} />}
      {call.kind === 'look' ? null : (
        <View style={styles.tiles}>
          {call.options.map((option) => (
            <Tile
              key={option.id}
              option={option}
              on={chosen.includes(option.id)}
              pick={call.pick === option.id}
              disabled={!canAnswer || draft.sending}
              onPress={() => {
                haptics.select();
                choose(store, call, option.id);
              }}>
              {notDoing && option.id === DO_NOT_DOING ? (
                <NotDoingNote value={draft.typed} onChange={(text) => edit(store, { typed: text })} onSubmit={go} editable={canAnswer} />
              ) : null}
            </Tile>
          ))}
        </View>
      )}
      {call.against ? (
        <Text style={styles.against} selectable>
          <Text style={{ color: colors.tertiary }}>{t(AGAINST_IT)} </Text>
          {call.against}
        </Text>
      ) : null}
      {runs ? (
        <View style={styles.extras}>
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
        </View>
      ) : null}
      {runs && draft.runsOpen ? (
        <Text style={styles.command} selectable>
          {runs}
        </Text>
      ) : null}
      {secret ? <Text style={styles.kept}>{kept(call.asker.name, t)}</Text> : null}
      {failed ? <Text style={styles.failed}>{pushFailed(failed, t)}</Text> : null}
      {hasGo || next ? (
        <View style={styles.goRow}>
          {next ? (
            <Pressable
              onPress={() => {
                haptics.select();
                router.replace(`/device/${core}/call/${next.id}`);
              }}
              style={({ pressed }) => [styles.next, pressed && { opacity: 0.6 }]}
              accessibilityRole="button">
              <Text style={styles.nextText}>{t(NEXT_CALL)}</Text>
            </Pressable>
          ) : null}
          {hasGo ? (
            <Pressable
              onPress={go}
              disabled={!ready}
              style={({ pressed }) => [styles.go, !ready && { opacity: 0.5 }, pressed && { opacity: 0.8 }]}
              accessibilityRole="button"
              accessibilityState={{ disabled: !ready }}>
              <Text style={styles.goText} numberOfLines={1}>
                {goWords}
              </Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
      <Why call={call} />
    </View>
  );
});

/** What the call rests on, read whole under the answer as on the Mac's page. */
function Why({ call }: { call: PersonCall }) {
  const styles = useStyles(makeStyles);
  const t = useT();
  const context = contextOf(call);
  const evidence = (call.evidence ?? []).filter((item) => !item.picture);
  if (!context && !evidence.length) return null;
  return (
    <View style={styles.why}>
      <SectionLabel>{t(WHY_IT_CAME_UP)}</SectionLabel>
      {context ? (
        <Text style={styles.context} selectable>
          {context}
        </Text>
      ) : null}
      {evidence.map((item, index) => (
        <Text key={index} style={styles.evidenceText} selectable>
          {item.what}
        </Text>
      ))}
    </View>
  );
}

/** One choice, a full-width tile; the chosen one lifts. */
function Tile({
  option,
  on,
  pick,
  disabled,
  onPress,
  children,
}: {
  option: CallOption;
  on: boolean;
  pick: boolean;
  disabled: boolean;
  onPress: () => void;
  children?: ReactNode;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  return (
    <View
      style={[
        styles.tile,
        on
          ? { backgroundColor: colors.active, borderColor: colors.borderStrong }
          : { backgroundColor: colors.overlay, borderColor: colors.border },
      ]}>
      <Pressable
        onPress={onPress}
        disabled={disabled}
        style={styles.option}
        accessibilityRole="radio"
        accessibilityState={{ checked: on, disabled }}>
        <ChoiceMark on={on} />
        <View style={styles.optionWords}>
          <Text style={styles.optionLabel}>{option.label}</Text>
          {option.detail ? <Text style={styles.optionDetail}>{option.detail}</Text> : null}
        </View>
        {pick ? (
          <View style={styles.pick}>
            <ChiefMark size={13} />
            <Text style={styles.pickText}>{t(PICK)}</Text>
          </View>
        ) : null}
      </Pressable>
      {children}
    </View>
  );
}

/** Why a do call is not being done, typed inside its Not doing tile. */
function NotDoingNote({
  value,
  onChange,
  onSubmit,
  editable,
}: {
  value: string;
  onChange: (text: string) => void;
  onSubmit: () => void;
  editable: boolean;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const t = useT();
  const [focused, setFocused] = useState(false);
  return (
    <View style={styles.note}>
      <Icon name="IconPencil" size={13} color={focused || value.trim() ? colors.tertiary : colors.inkFaint} />
      <TextInput
        value={value}
        onChangeText={onChange}
        placeholder={t(WHY_NOT_DOING)}
        placeholderTextColor={colors.tertiary}
        style={styles.input}
        returnKeyType="send"
        onSubmitEditing={onSubmit}
        editable={editable}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
      />
    </View>
  );
}

/** The ring of an option. */
function ChoiceMark({ on }: { on: boolean }) {
  const colors = useColors();
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

/** An agent the call names, as the pill that opens its chat: as the host lists it, or as the call names it. */
function Asker({ asker }: { asker: CallAsker }) {
  const crew = useCrew();
  const agent: Named = namedAgent(crew, asker.agentId) ?? {
    id: asker.agentId,
    written: `@${asker.name}`,
    chief: asker.chief === true,
    provider: asker.cli,
  };
  return <AgentPill agent={agent} size={13} />;
}

/** Where a do call's task is done, or what a look call shows: the link's words and its address; a tap opens it. */
function LinkTile({ link, onOpen }: { link: CallLink; onOpen: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <Pressable
      onPress={() => {
        openLink(link.url);
        onOpen();
      }}
      style={({ pressed }) => [styles.link, pressed && { backgroundColor: colors.active }]}
      accessibilityRole="link"
      accessibilityLabel={link.label}>
      <Icon name={serviceMark(link.url) ?? 'IconGlobe'} size={14} color={colors.secondary} />
      <Text style={styles.linkLabel} numberOfLines={1}>
        {link.label}
      </Text>
      {link.label !== link.url ? (
        <Text style={styles.linkUrl} numberOfLines={1}>
          {link.url}
        </Text>
      ) : null}
    </Pressable>
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
      <CallPicture path={call.options[lead].picture} style={styles.lead} whole />
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

/** A picture a call names on the host, fetched to this phone; evidence shows a page from its top, and the lead
 * option all of its picture. */
function CallPicture({ path, style, top, whole }: { path?: string; style: object; top?: boolean; whole?: boolean }) {
  const styles = useStyles(makeStyles);
  const uri = useHostFile(path ?? '', !!path, 1200);
  return (
    <View style={[styles.picture, style]}>
      {uri ? (
        <Image
          source={{ uri }}
          style={top ? styles.topImage : StyleSheet.absoluteFill}
          resizeMode={whole ? 'contain' : 'cover'}
          accessibilityIgnoresInvertColors
        />
      ) : null}
    </View>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    who: { flexDirection: 'row', alignItems: 'center', gap: 8 },
    sep: { color: colors.inkFaint },
    age: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary },
    openChat: { fontFamily: fonts.ui, fontSize: 14, color: colors.secondary },
    question: { marginTop: 14, fontFamily: fonts.uiSemibold, fontSize: 21, lineHeight: 27, color: colors.ink },
    prose: { marginTop: 6, fontFamily: fonts.ui, fontSize: 15, lineHeight: 21, color: colors.secondary },
    link: {
      marginTop: 12,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      minHeight: 44,
      paddingHorizontal: 14,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.composer,
    },
    linkLabel: { flexShrink: 0, maxWidth: '70%', fontFamily: fonts.uiMedium, fontSize: 14.5, color: colors.ink },
    linkUrl: { flex: 1, fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    tiles: { marginTop: 16, gap: 9 },
    tile: { borderRadius: 12, borderWidth: 1, overflow: 'hidden' },
    option: { flexDirection: 'row', alignItems: 'center', gap: 12, minHeight: 48, paddingHorizontal: 14, paddingVertical: 11 },
    optionWords: { flex: 1, gap: 2 },
    optionLabel: { fontFamily: fonts.ui, fontSize: 15.5, color: colors.ink },
    optionDetail: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    pick: { flexDirection: 'row', alignItems: 'center', gap: 5 },
    pickText: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    note: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      minHeight: 44,
      paddingHorizontal: 14,
      borderTopWidth: 1,
      borderTopColor: colors.border,
    },
    input: { flex: 1, fontFamily: fonts.ui, fontSize: 15, color: colors.ink, paddingVertical: 10 },
    against: { marginTop: 12, fontFamily: fonts.ui, fontSize: 13.5, color: colors.secondary },
    extras: { marginTop: 6, flexDirection: 'row', alignItems: 'center', minHeight: 32 },
    runs: { flexDirection: 'row', alignItems: 'center', gap: 6, minHeight: 32 },
    extraText: { fontFamily: fonts.ui, fontSize: 13, color: colors.tertiary },
    command: { fontFamily: fonts.mono, fontSize: 12.5, color: colors.secondary },
    kept: { marginTop: 8, fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    failed: { marginTop: 10, fontFamily: fonts.ui, fontSize: 13, color: colors.danger },
    goRow: { marginTop: 14, flexDirection: 'row', justifyContent: 'flex-end', alignItems: 'center', gap: 8 },
    next: { minHeight: 40, paddingHorizontal: 14, justifyContent: 'center' },
    nextText: { fontFamily: fonts.uiMedium, fontSize: 14.5, color: colors.secondary },
    go: { flexShrink: 1, minHeight: 40, paddingHorizontal: 16, borderRadius: 10, backgroundColor: colors.accent, justifyContent: 'center' },
    goText: { fontFamily: fonts.uiSemibold, fontSize: 15, color: colors.ground },
    why: { marginTop: 22, gap: 8 },
    context: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 21, color: colors.secondary },
    evidenceText: { fontFamily: fonts.ui, fontSize: 14, lineHeight: 19, color: colors.tertiary },
    strip: { marginTop: 12, height: 160, flexDirection: 'row', gap: 6 },
    lead: { flex: 2 },
    rest: { width: 104, gap: 6 },
    restItem: { flex: 1 },
    fill: { flex: 1 },
    evidence: { marginTop: 12, height: 150, flexDirection: 'row', gap: 6 },
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
