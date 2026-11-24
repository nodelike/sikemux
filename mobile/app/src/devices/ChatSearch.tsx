import { useEffect, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { router } from 'expo-router';

import { CHIEF_ROLE } from '@shared/calls/words';
import { formatAge } from '@shared/lib/format';
import type { AgentShown, FoundChat, FoundLine, ProjectInfo, RecentChats, RecentInfo } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem as problemOf } from '@/core/problem';
import type { Snapshot } from '@/core/protocol';
import { Working } from '@/devices/AgentMarks';
import { OwnMark } from '@/devices/Crew';
import { connectionOf, useLive } from '@/devices/hub';
import { askFind } from '@/devices/pendingFind';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { useMinute } from '@/ui/motion';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Row, Rows } from '@/ui/list';
import { EmptyState } from '@/ui/prompt';
import { fonts, keyboardFor, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';

/** How long typing rests before the host is asked. */
const TYPING_REST_MS = 150;
const MOST = 50;

/** What the host found for the words typed, its rows in the order it sent them; `complete` once every answer came. */
type Found = { words: string; found: FoundChat[]; chats: Map<string, RecentInfo>; said: Record<string, FoundLine>; complete: boolean };

const keyOf = (found: FoundChat) => found.agentId ?? found.chat ?? '';

const foundOf = (words: string, answer: RecentChats, complete: boolean): Found => ({
  words,
  found: answer.found ?? [],
  chats: new Map(answer.chats.map((chat) => [chat.id, chat])),
  said: answer.said ?? {},
  complete,
});

/** The later answer's rows go under those already shown, leaving every one of them where it is. */
const adding = (shown: Found, answer: RecentChats): Found => {
  const here = new Set(shown.found.map(keyOf));
  return {
    words: shown.words,
    found: [...shown.found, ...(answer.found ?? []).filter((found) => !here.has(keyOf(found)))],
    chats: new Map([...answer.chats.map((chat) => [chat.id, chat] as const), ...shown.chats]),
    said: { ...shown.said, ...answer.said },
    complete: true,
  };
};

/**
 * The host's chats searched by their agents' names, their titles and what was said in them, in the order the host
 * ranks them, each found by its words with the line that holds them.
 */
export function ChatSearch({
  core,
  snapshot,
  scope,
  full,
  hostName,
  bottom,
  onClose,
}: {
  core: string;
  snapshot: Snapshot;
  scope?: ProjectInfo;
  full: boolean;
  hostName: string;
  bottom: number;
  onClose: () => void;
}) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const t = useT();
  const live = useLive(core);
  const [query, setQuery] = useState('');
  const [answered, setAnswered] = useState<Found>();
  const minute = useMinute();
  const [resuming, setResuming] = useState<string>();
  const [note, setNote] = useState<{ text: string; failed: boolean }>();
  const words = query.trim().toLowerCase();
  const connection = connectionOf(live);

  useEffect(() => {
    if (!words || !connection) return;
    let current = true;
    const ask = (quick: boolean) =>
      callHost(connection, 'recentChats', { project: scope?.id ?? null, query: words, cursor: null, limit: MOST, quick });
    const namesOnly = words.startsWith('@');
    ask(true)
      .then(
        (answer) =>
          current && setAnswered((shown) => (shown?.words === words && shown.complete ? shown : foundOf(words, answer, namesOnly))),
      )
      .catch(() => undefined);
    const timer = namesOnly
      ? undefined
      : setTimeout(() => {
          ask(false)
            .then(
              (answer) =>
                current && setAnswered((shown) => (shown?.words === words ? adding(shown, answer) : foundOf(words, answer, true))),
            )
            .catch(() => undefined);
        }, TYPING_REST_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [words, connection, scope?.id]);

  const found = answered?.words === words ? answered : undefined;
  const said = found?.said ?? {};
  const agentOf = new Map([...snapshot.inbox.agents, ...(snapshot.inbox.kept ?? [])].map((agent) => [agent.agentId, agent]));
  type Hit = { found: FoundChat; agent?: AgentShown; chat?: RecentInfo };
  const hits: Hit[] = (found?.found ?? []).flatMap((hit): Hit[] => {
    if (hit.agentId) {
      const agent = agentOf.get(hit.agentId);
      return agent && !agent.session ? [{ found: hit, agent }] : [];
    }
    const chat = hit.chat ? found?.chats.get(hit.chat) : undefined;
    return chat ? [{ found: hit, chat }] : [];
  });
  const firstSaid = hits.findIndex((hit) => hit.found.by === 'said');
  const groups = (firstSaid < 0 ? [hits] : [hits.slice(0, firstSaid), hits.slice(firstSaid)]).filter((group) => group.length);

  const open = (agentId: string, line: FoundLine | undefined) => {
    haptics.tap();
    askFind(core, agentId, line);
    router.push(`/device/${core}/chat/${agentId}`);
  };
  const resume = async (chat: RecentInfo) => {
    if (resuming) return;
    if (!full) {
      haptics.warning();
      setNote({ text: t('This phone can only watch {host}.', { host: hostName }), failed: false });
      return;
    }
    if (live.status !== 'open') {
      haptics.failure();
      setNote({ text: t('Not connected to the host.'), failed: true });
      return;
    }
    haptics.tap();
    setNote(undefined);
    setResuming(chat.id);
    try {
      const agentId = await live.connection.resumeChat(chat.id);
      askFind(core, agentId, said[chat.id]);
      router.push(`/device/${core}/chat/${agentId}`);
    } catch (error) {
      haptics.failure();
      setNote({ text: problemOf(error), failed: true });
    } finally {
      setResuming(undefined);
    }
  };

  return (
    <View style={styles.wrap}>
      <View style={styles.head}>
        <View style={[styles.search, { borderColor: colors.borderSelected }]}>
          <Icon name="IconSearch" size={15} color={colors.tertiary} />
          <TextInput
            value={query}
            onChangeText={setQuery}
            placeholder={t('Search chats…')}
            placeholderTextColor={colors.tertiary}
            style={styles.searchInput}
            autoFocus
            autoCorrect={false}
            autoCapitalize="none"
            keyboardAppearance={keyboardFor(colors)}
            returnKeyType="search"
          />
        </View>
        <Pressable onPress={onClose} hitSlop={8} accessibilityRole="button">
          <Text style={styles.cancel}>{t('Cancel')}</Text>
        </Pressable>
      </View>
      <ScrollView
        keyboardShouldPersistTaps="handled"
        keyboardDismissMode="on-drag"
        showsVerticalScrollIndicator={false}
        contentContainerStyle={[styles.body, { paddingBottom: bottom + 24 }]}>
        {groups.map((group) => (
          <View key={keyOf(group[0].found)} style={styles.group}>
            <Rows>
              {group.map(({ found: hit, agent, chat }) => {
                const line = hit.chat ? said[hit.chat] : undefined;
                return agent ? (
                  <Row
                    key={agent.agentId}
                    mark={<OwnMark agentId={agent.agentId} provider={agent.cli} chief={agent.chief} size={20} still />}
                    title={agent.chief ? t(CHIEF_ROLE) : agent.title}
                    name={agent.name}
                    under={line ? <FoundLineText line={line} /> : undefined}
                    onPress={() => open(agent.agentId, line)}
                  />
                ) : chat ? (
                  <Row
                    key={chat.id}
                    mark={<AgentIcon provider={chat.provider} size={20} />}
                    title={chat.title}
                    under={line ? <FoundLineText line={line} /> : undefined}
                    end={
                      resuming === chat.id ? (
                        <Working />
                      ) : (
                        <Text style={[type.meta, styles.age]}>{formatAge(Number(chat.activeAt), minute * 60_000)}</Text>
                      )
                    }
                    onPress={() => void resume(chat)}
                  />
                ) : null;
              })}
            </Rows>
          </View>
        ))}
        {note ? <Text style={[styles.note, note.failed && styles.noteFailed]}>{note.text}</Text> : null}
        {words && found?.complete && !hits.length ? <EmptyState icon="IconSearch" text={t('No chats match.')} /> : null}
      </ScrollView>
    </View>
  );
}

/** The line a chat said the words in: the quiet ink, the words themselves in full ink, as the host cut it. */
export function FoundLineText({ line }: { line: FoundLine }) {
  const styles = useStyles(makeStyles);
  return (
    <>
      {line.runs.map((run, index) => (
        <Text key={index} style={run.found ? styles.found : undefined}>
          {run.text}
        </Text>
      ))}
    </>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    wrap: { flex: 1 },
    head: { flexDirection: 'row', alignItems: 'center', gap: 12, paddingHorizontal: 16, paddingTop: 6, paddingBottom: 10 },
    search: {
      flex: 1,
      flexDirection: 'row',
      alignItems: 'center',
      gap: 8,
      height: 40,
      paddingHorizontal: 12,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.sunken,
    },
    searchInput: { flex: 1, fontFamily: fonts.ui, fontSize: 15.5, color: colors.ink },
    cancel: { fontFamily: fonts.ui, fontSize: 15.5, color: colors.accent },
    body: { paddingHorizontal: 16 },
    group: { marginBottom: 14 },
    found: { fontFamily: fonts.uiMedium, color: colors.ink },
    age: { fontSize: 12, fontVariant: ['tabular-nums'] },
    note: { ...type.meta, fontSize: 12.5, paddingHorizontal: 8, paddingTop: 8 },
    noteFailed: { color: colors.danger },
  });
};
