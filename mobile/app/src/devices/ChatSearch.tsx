import { useEffect, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { router } from 'expo-router';

import { formatAge } from '@shared/lib/format';
import type { FoundLine, ProjectInfo, RecentChats, RecentInfo } from '@shared/wire.generated';
import { callHost } from '@/core/hostCalls';
import { problem as problemOf } from '@/core/problem';
import type { Snapshot } from '@/core/protocol';
import { Working } from '@/devices/AgentMarks';
import { connectionOf, useLive } from '@/devices/hub';
import { askFind } from '@/devices/pendingFind';
import { useT } from '@/i18n';
import { haptics } from '@/ui/haptics';
import { useMinute } from '@/ui/motion';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Row, Rows, SectionLabel } from '@/ui/list';
import { EmptyState } from '@/ui/prompt';
import { fonts, keyboardFor, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';

/** How long typing rests before the host is asked. */
const TYPING_REST_MS = 150;
const MOST = 50;

/**
 * The host's chats searched by their titles and by what was said in them: the open agents narrowed here, and the
 * saved chats the host finds, each found by its words with the line that holds them.
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
  const [answered, setAnswered] = useState<{ words: string; answer: RecentChats }>();
  const minute = useMinute();
  const [resuming, setResuming] = useState<string>();
  const [note, setNote] = useState<{ text: string; failed: boolean }>();
  const words = query.trim().toLowerCase();
  const connection = connectionOf(live);

  useEffect(() => {
    if (!words || !connection) return;
    let current = true;
    const timer = setTimeout(() => {
      callHost(connection, 'recentChats', { project: scope?.id ?? null, query: words, cursor: null, limit: MOST })
        .then((answer) => current && setAnswered({ words, answer }))
        .catch(() => undefined);
    }, TYPING_REST_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [words, connection, scope?.id]);

  const found = answered?.words === words ? answered.answer : undefined;
  const said = found?.said ?? {};
  const chatOf = new Map(snapshot.recent.filter((chat) => chat.agentId).map((chat) => [chat.agentId as string, chat.id]));
  const lineOf = (agentId: string) => {
    const chat = chatOf.get(agentId);
    return chat ? said[chat] : undefined;
  };
  const agents = words
    ? snapshot.inbox.agents.filter(
        (agent) =>
          (!scope || agent.project === scope.id) &&
          !agent.session &&
          (agent.title.toLowerCase().includes(words) || lineOf(agent.agentId) !== undefined),
      )
    : [];
  const saved = (found?.chats ?? []).filter((chat) => !chat.agentId);

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
        {agents.length ? (
          <Rows>
            {agents.map((agent) => {
              const line = lineOf(agent.agentId);
              return (
                <Row
                  key={agent.agentId}
                  mark={<AgentIcon provider={agent.cli} size={20} />}
                  title={agent.title}
                  name={agent.name}
                  under={line ? <FoundLineText line={line} /> : undefined}
                  onPress={() => open(agent.agentId, line)}
                />
              );
            })}
          </Rows>
        ) : null}
        {saved.length ? (
          <>
            <SectionLabel>Recent</SectionLabel>
            <Rows>
              {saved.map((chat) => {
                const line = said[chat.id];
                return (
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
                );
              })}
            </Rows>
          </>
        ) : null}
        {note ? <Text style={[styles.note, note.failed && styles.noteFailed]}>{note.text}</Text> : null}
        {words && found && !agents.length && !saved.length ? <EmptyState icon="IconSearch" text={t('No chats match.')} /> : null}
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
    found: { fontFamily: fonts.uiMedium, color: colors.ink },
    age: { fontSize: 12, fontVariant: ['tabular-nums'] },
    note: { ...type.meta, fontSize: 12.5, paddingHorizontal: 8, paddingTop: 8 },
    noteFailed: { color: colors.danger },
  });
};
