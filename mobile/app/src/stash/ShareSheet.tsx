import { useEffect, useRef, useState } from 'react';
import { Image, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';

import { formatBytes } from '@shared/lib/format';
import type { TokenSource } from '@/account/api';
import { useT } from '@/i18n';
import { problem as problemOf } from '@/core/problem';
import { Button, Chip } from '@/ui/controls';
import { haptics } from '@/ui/haptics';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { PromptGlyph } from '@/ui/prompt';
import { useBottomGap } from '@/ui/screen';
import { fonts, type Palette, translucent, useColors, useStyles } from '@/ui/theme';
import type { Shared } from '../../modules/stash';
import type { StashTarget } from './api';
import type { ShareChat, ShareChats } from './chats';
import { outgoingOf, stashAll, tooBig, type Outgoing } from './sharing';
import { siteOf } from './words';

/** How long the confirmation shows before the sheet goes. */
const DONE_MS = 900;
const PHOTOS_SHOWN = 4;

type Phase =
  { at: 'ready' } | { at: 'sending'; share: number } | { at: 'done'; title: string; detail: string } | { at: 'failed'; problem: string };

/**
 * What another app shared, with a note, the chats used last and Stash. A chat gets it in its draft,
 * unsent; Stash keeps it for later. `onDone` closes the sheet.
 */
export function ShareSheet({
  shared,
  chats,
  token,
  onDone,
}: {
  shared: Shared;
  chats: ShareChats;
  token: TokenSource;
  onDone: () => void;
}) {
  const t = useT();
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const bottom = useBottomGap();
  const [items] = useState(() => outgoingOf(shared));
  const [note, setNote] = useState('');
  const [phase, setPhase] = useState<Phase>({ at: 'ready' });
  const [last, setLast] = useState<{ target?: StashTarget; done: Phase }>();
  const cancel = useRef<AbortController | null>(null);
  const big = items.filter(tooBig);
  const sendable = items.filter((item) => !tooBig(item));

  useEffect(() => {
    if (phase.at !== 'done') return;
    const leaving = setTimeout(onDone, DONE_MS);
    return () => clearTimeout(leaving);
  }, [phase, onDone]);

  useEffect(() => () => cancel.current?.abort(), []);

  const send = async (target: StashTarget | undefined, done: Phase) => {
    if (!sendable.length) return;
    haptics.tap();
    setLast({ target, done });
    const abort = new AbortController();
    cancel.current = abort;
    setPhase({ at: 'sending', share: 0 });
    try {
      await stashAll(
        token,
        sendable,
        { note: note.trim() || undefined, target },
        (share) => setPhase({ at: 'sending', share }),
        abort.signal,
      );
      haptics.success();
      setPhase(done);
    } catch (error) {
      if (abort.signal.aborted) return;
      haptics.failure();
      setPhase({ at: 'failed', problem: problemOf(error) });
    }
  };

  const toChat = (chat: ShareChat) =>
    void send(
      { hostKey: chat.host, chatId: chat.chatId },
      {
        at: 'done',
        title: chat.name ? t("Added to {name}'s draft", { name: chat.name }) : t("Added to the chat's draft"),
        detail: t('Unsent, on {host}', { host: chat.hostName }),
      },
    );
  const newChat = chats.newChat;
  const toNewChat = () =>
    newChat &&
    void send(
      { hostKey: newChat.host, projectPath: newChat.projectPath, newChat: true },
      {
        at: 'done',
        title: t('In a new chat in {project}', { project: newChat.project }),
        detail: t('Unsent, on {host}', { host: newChat.hostName }),
      },
    );
  const toStash = () => void send(undefined, { at: 'done', title: t('Stashed'), detail: t('Attach it from + in any chat.') });
  const close = () => {
    cancel.current?.abort();
    onDone();
  };

  const head = (
    <View style={styles.head}>
      <Pressable onPress={close} hitSlop={8} style={styles.close} accessibilityRole="button" accessibilityLabel={t('Cancel')}>
        <Icon name="IconClose" size={16} color={colors.tertiary} />
      </Pressable>
      <Text style={styles.title} accessibilityRole="header">
        Sikemux
      </Text>
      <View style={styles.close} />
    </View>
  );

  if (phase.at === 'done' || phase.at === 'failed') {
    const failed = phase.at === 'failed';
    return (
      <View style={styles.wrap}>
        {head}
        <View style={styles.settled}>
          <View style={failed ? undefined : styles.doneGlyph}>
            {failed ? <PromptGlyph icon="IconGlobe" small /> : <Icon name="IconCheck" size={22} color={colors.live} />}
          </View>
          <Text style={styles.settledTitle}>{failed ? t("Couldn't stash this") : phase.title}</Text>
          <Text style={styles.settledText}>{failed ? phase.problem : phase.detail}</Text>
          {failed && last ? (
            <View style={{ marginTop: 12 }}>
              <Chip title={t('Try again')} onPress={() => void send(last.target, last.done)} />
            </View>
          ) : null}
        </View>
      </View>
    );
  }

  const sending = phase.at === 'sending';
  return (
    <View style={styles.wrap}>
      {head}
      <ScrollView
        style={styles.scroll}
        contentContainerStyle={styles.pad}
        keyboardShouldPersistTaps="handled"
        showsVerticalScrollIndicator={false}>
        <Preview items={items} />
        {sending ? (
          <View style={styles.progress}>
            <View style={[styles.progressDone, { width: `${Math.round(phase.share * 100)}%` }]} />
          </View>
        ) : null}
        {big.length ? (
          <Text style={styles.big}>
            {big.length === 1
              ? t('{name} is over 25 MB, too big to stash', { name: big[0].kind === 'file' ? big[0].file.name : t('1 file') })
              : t('{count} files are over 25 MB, too big to stash', { count: big.length })}
          </Text>
        ) : null}
        <View style={[sending && styles.dim]} pointerEvents={sending ? 'none' : 'auto'}>
          <TextInput
            value={note}
            onChangeText={setNote}
            placeholder={t('Add a note')}
            placeholderTextColor={colors.tertiary}
            multiline
            maxLength={2000}
            style={styles.note}
            selectionColor={colors.accent}
          />
          {chats.chats.length || newChat ? (
            <>
              <SectionLabel>{t('To a chat')}</SectionLabel>
              <Rows>
                {chats.chats.map((chat) => (
                  <Row
                    key={`${chat.host}:${chat.chatId}`}
                    mark={<AgentIcon provider={chat.provider} size={20} />}
                    title={chat.title ?? t('New chat')}
                    place={chat.project}
                    detail={chat.hostName}
                    name={chat.name}
                    onPress={sendable.length ? () => toChat(chat) : undefined}
                  />
                ))}
                {newChat ? (
                  <Row
                    mark={<Icon name="IconPlus" size={18} color={colors.secondary} />}
                    title={t('New chat in {project}', { project: newChat.project })}
                    detail={newChat.hostName}
                    onPress={sendable.length ? toNewChat : undefined}
                  />
                ) : null}
              </Rows>
            </>
          ) : null}
        </View>
      </ScrollView>
      <View style={[styles.foot, { paddingBottom: bottom }]}>
        <Button
          kind="primary"
          icon="IconStack"
          title={sending ? t('Stashing') : t('Stash')}
          busy={sending}
          disabled={!sendable.length}
          onPress={toStash}
        />
      </View>
    </View>
  );
}

function Preview({ items }: { items: readonly Outgoing[] }) {
  const t = useT();
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const files = items.flatMap((item) => (item.kind === 'file' ? [item.file] : []));
  const link = items.find((item) => item.kind === 'link');
  const text = items.find((item) => item.kind === 'text');
  if (files.length) {
    const pictures = files.filter((file) => file.uri && file.mime.startsWith('image/'));
    const size = files.reduce((sum, file) => sum + file.size, 0);
    const named =
      files.length === 1
        ? files[0].name
        : pictures.length === files.length
          ? t('{count} photos', { count: files.length })
          : t('{count} files', { count: files.length });
    return (
      <View style={[styles.card, styles.flat]}>
        {pictures.length ? (
          <View style={styles.photos}>
            {pictures.slice(0, PHOTOS_SHOWN).map((file) => (
              <Image key={file.uri} source={{ uri: file.uri! }} style={styles.photo} resizeMode="cover" resizeMethod="resize" />
            ))}
          </View>
        ) : (
          <View style={styles.fileFace}>
            <Icon name={files[0].mime.startsWith('video/') ? 'IconImage' : 'IconFile'} size={20} color={colors.secondary} />
          </View>
        )}
        <View style={styles.words}>
          <Text style={[styles.cardTitle, files.length === 1 && !pictures.length && styles.mono]} numberOfLines={2}>
            {named}
          </Text>
          <Text style={styles.cardSub}>{formatBytes(size)}</Text>
        </View>
      </View>
    );
  }
  if (link?.kind === 'link') {
    return (
      <View style={[styles.card, styles.flat]}>
        <View style={styles.fileFace}>
          <Icon name="IconLink" size={20} color={colors.secondary} />
        </View>
        <View style={styles.words}>
          <Text style={styles.cardTitle} numberOfLines={2}>
            {link.title ?? link.url.replace(/^https?:\/\//, '')}
          </Text>
          <View style={styles.site}>
            <Icon name="IconGlobe" size={12} color={colors.tertiary} />
            <Text style={styles.cardSub} numberOfLines={1}>
              {siteOf(link.url)}
            </Text>
          </View>
        </View>
      </View>
    );
  }
  return (
    <View style={[styles.card, { padding: 14 }]}>
      <Text style={styles.quote} numberOfLines={6}>
        {text?.kind === 'text' ? text.text : ''}
      </Text>
    </View>
  );
}

const makeStyles = (colors: Palette) =>
  StyleSheet.create({
    wrap: { flex: 1 },
    head: { flexDirection: 'row', alignItems: 'center', height: 46, paddingHorizontal: 10 },
    close: { width: 40, height: 40, alignItems: 'center', justifyContent: 'center' },
    title: { flex: 1, textAlign: 'center', fontFamily: fonts.uiSemibold, fontSize: 16, color: colors.ink },
    scroll: { flex: 1 },
    pad: { paddingHorizontal: 16, paddingTop: 4, paddingBottom: 16 },
    card: { borderRadius: 14, borderWidth: 1, borderColor: colors.border, backgroundColor: colors.raised, overflow: 'hidden' },
    flat: { flexDirection: 'row', alignItems: 'center', gap: 12, padding: 10 },
    photos: { flexDirection: 'row', gap: 6 },
    photo: { width: 72, height: 72, borderRadius: 10, backgroundColor: colors.overlay },
    fileFace: {
      width: 52,
      height: 52,
      borderRadius: 10,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
      alignItems: 'center',
      justifyContent: 'center',
    },
    words: { flex: 1, minWidth: 0, gap: 4 },
    cardTitle: { fontFamily: fonts.uiSemibold, fontSize: 14.5, lineHeight: 19, color: colors.ink },
    mono: { fontFamily: fonts.mono, fontSize: 13, fontWeight: 'normal' },
    cardSub: { fontFamily: fonts.ui, fontSize: 12.5, color: colors.tertiary },
    site: { flexDirection: 'row', alignItems: 'center', gap: 6 },
    quote: { fontFamily: fonts.ui, fontSize: 14.5, lineHeight: 21, color: colors.secondary },
    progress: {
      height: 3,
      marginTop: -3,
      marginHorizontal: 1,
      borderBottomLeftRadius: 14,
      borderBottomRightRadius: 14,
      overflow: 'hidden',
      backgroundColor: colors.border,
    },
    progressDone: { height: 3, backgroundColor: colors.ink },
    big: { marginTop: 8, fontFamily: fonts.ui, fontSize: 13.5, color: colors.danger },
    dim: { opacity: 0.45 },
    note: {
      marginTop: 10,
      minHeight: 44,
      maxHeight: 120,
      paddingHorizontal: 14,
      paddingTop: 11,
      paddingBottom: 11,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
      fontFamily: fonts.ui,
      fontSize: 15,
      lineHeight: 20,
      color: colors.ink,
    },
    foot: { paddingHorizontal: 16, paddingTop: 10, borderTopWidth: 1, borderTopColor: colors.border },
    settled: { flex: 1, alignItems: 'center', justifyContent: 'center', gap: 6, paddingHorizontal: 32, paddingBottom: 80 },
    doneGlyph: {
      width: 44,
      height: 44,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: translucent(colors.live, 0.4),
      backgroundColor: translucent(colors.live, 0.07),
      alignItems: 'center',
      justifyContent: 'center',
      marginBottom: 12,
    },
    settledTitle: { fontFamily: fonts.uiSemibold, fontSize: 19, letterSpacing: -0.4, color: colors.ink, textAlign: 'center' },
    settledText: { fontFamily: fonts.ui, fontSize: 14, color: colors.tertiary, textAlign: 'center' },
  });
