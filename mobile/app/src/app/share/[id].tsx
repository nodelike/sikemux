import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import * as Linking from 'expo-linking';
import { getClerkInstance } from '@clerk/expo';
import { FlashList, type FlashListRef } from '@shopify/flash-list';
import { SafeAreaView } from 'react-native-safe-area-context';

import { agentLabel } from '@shared/agents/catalog';
import { activityText } from '@shared/chat/chatStatus';
import { activeToolLabel } from '@shared/chat/toolLabels';
import type { ChatMessage } from '@shared/chat/types';
import { parseShareLink, type ShareLink } from '@shared/share/link';
import { sharedChatTitle, ShareSession, type ShareSnapshot } from '@shared/share/session';
import { apiUrl } from '@/account/config';
import { Folds, FoldsContext, rowFold } from '@/chat/folds';
import { SharedFilesContext } from '@/chat/hostFiles';
import { ToolDetailContext } from '@/chat/Tools';
import { useTranscriptScroll } from '@/chat/transcriptScroll';
import { Activity, Earlier, EarlierLost, Message, ProviderContext, SubagentsContext, subagentsOf, WorkSummary } from '@/chat/Transcript';
import { phoneCipher } from '@/share/cipher';
import { AgentIcon, Icon } from '@/ui/Icon';
import { useMinute } from '@/ui/motion';
import { EmptyState, type Glyph } from '@/ui/prompt';
import { Nav, Screen } from '@/ui/screen';
import { Working } from '@/ui/status';
import { fonts, type Palette, useColors, useStyles, useType } from '@/ui/theme';

type Problem = { icon: Glyph; text: string };

const PROBLEMS: Partial<Record<ShareSnapshot['status'], Problem>> = {
  missing: { icon: 'IconPower', text: 'This chat is not running on its host right now.' },
  notFound: { icon: 'IconUnlink', text: 'No chat is shared at this link.' },
  signedOut: { icon: 'IconUser', text: 'Sign in to Sikemux to watch shared chats.' },
  unreadable: { icon: 'IconUnlink', text: 'This link does not open the chat it points at. Check that the whole link was copied.' },
};

function problemOf(snapshot: ShareSnapshot): Problem | undefined {
  if (snapshot.status === 'ended')
    return snapshot.endedBecause === 'expired'
      ? { icon: 'IconHourglass', text: 'This share has expired.' }
      : { icon: 'IconStop', text: 'Sharing this chat has stopped.' };
  return PROBLEMS[snapshot.status];
}

function endsIn(expiresAt: number, now: number): string {
  const minutes = Math.max(1, Math.round((expiresAt - now) / 60_000));
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return hours === 1 ? '1 hour' : `${hours} hours`;
  return `${Math.round(hours / 24)} days`;
}

/** The key is after `#`, which a link that opened the app keeps; a link opened from inside it passes the key along. */
function useShareLink(): ShareLink | null {
  const { id, key } = useLocalSearchParams<{ id: string; key?: string }>();
  const opened = Linking.useLinkingURL();
  return useMemo(() => {
    if (key) return parseShareLink(`sikemux://share/${id}#${key}`);
    const link = opened ? parseShareLink(opened) : null;
    return link?.id === id ? link : null;
  }, [id, key, opened]);
}

export default function SharedChatScreen() {
  const link = useShareLink();
  if (!link)
    return (
      <Screen>
        <Nav title={<Text>Shared chat</Text>} />
        <Missing problem={{ icon: 'IconUnlink', text: 'This link is incomplete. Copy the whole link.' }} />
      </Screen>
    );
  return <Watching key={link.id} link={link} />;
}

function Missing({ problem }: { problem: Problem }) {
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.alone}>
      <EmptyState icon={problem.icon} text={problem.text} />
    </View>
  );
}

/** A token from the phone's sign-in, read when the share connects rather than when the screen draws. */
async function signInToken(fresh: boolean): Promise<string | null> {
  return (await getClerkInstance().session?.getToken(fresh ? { skipCache: true } : undefined)) ?? null;
}

function Watching({ link }: { link: ShareLink }) {
  const [session] = useState(
    () =>
      new ShareSession({
        link,
        url: `${apiUrl().replace(/^http/, 'ws')}/v1/share`,
        cipher: phoneCipher(link),
        token: signInToken,
      }),
  );
  useEffect(() => {
    session.start();
    return () => session.stop();
  }, [session]);
  return <SharedChat session={session} />;
}

function SharedChat({ session }: { session: ShareSession }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const snapshot = useSyncExternalStore(session.subscribe, session.snapshot);
  const state = snapshot.agent;
  const provider = snapshot.chat?.provider ?? 'agent';
  const title = sharedChatTitle(snapshot) ?? `${agentLabel(provider)} chat`;
  const [folds] = useState(() => new Folds());
  const [openWork, setOpenWork] = useState<ReadonlySet<string>>(() => new Set());
  const workOpen = (id: string) => openWork.has(id);
  const subagents = useMemo(() => subagentsOf(state.messages, state.subagentRows), [state.messages, state.subagentRows]);
  const toggleWork = (id: string) =>
    setOpenWork((shown) => {
      const next = new Set(shown);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const liveId = state.running ? state.messages[state.messages.length - 1]?.id : undefined;
  const marks = useMemo(() => [liveId, openWork], [liveId, openWork]);
  const [since] = useState(Date.now);
  const now = useMinute() * 60_000;
  const live = snapshot.status === 'live';
  const activity = live ? activityText(state, activeToolLabel(state)) : null;
  const problem = problemOf(snapshot);
  const list = useRef<FlashListRef<ChatMessage>>(null);
  const scroll = useTranscriptScroll(list, {
    has: snapshot.hasEarlier,
    idle: snapshot.earlier === 'idle',
    load: session.loadEarlier,
  });
  const noTap = () => {};
  const noRetry = () => {};
  const emptyFiles = useMemo(() => new Map(), []);
  const emptyUnsent = useMemo(() => new Map(), []);
  const footer = !problem
    ? snapshot.hostOnline
      ? `Read-only${snapshot.expiresAt ? ` · link ends in ${endsIn(snapshot.expiresAt, now)}` : ''}`
      : 'Read-only · the sharer is offline'
    : problem.text;

  return (
    <Screen>
      <Nav
        title={
          <>
            <AgentIcon provider={provider} size={17} />
            <Text style={styles.title} numberOfLines={1} accessibilityRole="header">
              {title}
            </Text>
          </>
        }
      />
      {!state.messages.length ? (
        problem ? (
          <Missing problem={problem} />
        ) : (
          <View style={styles.attaching} accessibilityLiveRegion="polite">
            <Working />
            <Text style={type.meta}>Opening the shared chat…</Text>
          </View>
        )
      ) : (
        <SharedFilesContext value={session.readImage}>
          <ProviderContext value={provider}>
            <FoldsContext value={folds}>
              <SubagentsContext value={subagents}>
                <ToolDetailContext value={session.readToolDetail}>
                  <View style={styles.transcript}>
                    <FlashList
                      ref={list}
                      showsVerticalScrollIndicator={false}
                      data={state.messages}
                      keyExtractor={(message) => message.id}
                      getItemType={(message) => (rowFold(message, workOpen).hidden ? 'folded' : message.role)}
                      extraData={marks}
                      renderItem={({ item }) => {
                        const { summary: fold, hidden, from } = rowFold(item, workOpen);
                        const summary = fold ? (
                          <WorkSummary
                            took={fold.took}
                            calls={fold.calls}
                            thought={fold.thought}
                            open={workOpen(fold.id)}
                            onToggle={() => toggleWork(fold.id)}
                          />
                        ) : null;
                        if (hidden) return summary ?? <View />;
                        return (
                          <>
                            {summary}
                            <Message
                              message={item}
                              live={item.id === liveId}
                              unsent={emptyUnsent.get(item.id)}
                              onRetry={noRetry}
                              onTap={noTap}
                              sentFiles={emptyFiles}
                              from={from}
                            />
                          </>
                        );
                      }}
                      ListHeaderComponent={
                        snapshot.hasEarlier ? (
                          <Earlier
                            failed={snapshot.earlier === 'failed'}
                            loading={snapshot.earlier === 'loading'}
                            onRetry={session.loadEarlier}
                          />
                        ) : state.earlierLost ? (
                          <EarlierLost />
                        ) : null
                      }
                      ListFooterComponent={
                        <>
                          {state.permissions.map((request) => (
                            <Text key={request.requestId} style={styles.waiting}>
                              Waiting for permission: {request.toolCall.title}
                            </Text>
                          ))}
                          {activity ? <Activity provider={provider} label={activity} since={since} /> : null}
                          {state.error ? <Text style={styles.error}>{state.error}</Text> : null}
                        </>
                      }
                      contentContainerStyle={styles.content}
                      {...scroll.listProps}
                    />
                    {scroll.away ? (
                      <Pressable
                        onPress={scroll.toEnd}
                        style={styles.jump}
                        hitSlop={8}
                        accessibilityRole="button"
                        accessibilityLabel="Jump to latest message">
                        <Icon name="IconArrowDown" size={16} color={colors.inkDim} />
                      </Pressable>
                    ) : null}
                  </View>
                </ToolDetailContext>
              </SubagentsContext>
            </FoldsContext>
          </ProviderContext>
        </SharedFilesContext>
      )}
      <SafeAreaView edges={['bottom']} style={styles.wrap}>
        <View style={styles.readOnly}>
          <Icon name="IconEye" size={17} color={colors.inkDim} />
          <Text style={styles.readOnlyText}>{footer}</Text>
        </View>
      </SafeAreaView>
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  return StyleSheet.create({
    title: { fontFamily: fonts.uiSemibold, fontSize: 16, letterSpacing: -0.25, color: colors.ink, flexShrink: 1 },
    transcript: { flex: 1 },
    content: { paddingHorizontal: 18, paddingTop: 6, paddingBottom: 28 },
    alone: { flex: 1, justifyContent: 'center', paddingBottom: 80 },
    attaching: { flex: 1, flexDirection: 'row', alignItems: 'flex-end', justifyContent: 'center', gap: 10, paddingVertical: 24 },
    waiting: { fontFamily: fonts.ui, fontSize: 13.5, color: colors.tertiary, marginTop: 10 },
    error: { fontFamily: fonts.ui, fontSize: 13.5, color: colors.danger, marginTop: 10 },
    jump: {
      position: 'absolute',
      bottom: 8,
      alignSelf: 'center',
      width: 34,
      height: 34,
      borderRadius: 17,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.composer,
      alignItems: 'center',
      justifyContent: 'center',
    },
    wrap: { paddingHorizontal: 10, paddingTop: 8, paddingBottom: 6 },
    readOnly: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      padding: 14,
      borderRadius: 16,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.composer,
    },
    readOnlyText: { flex: 1, fontFamily: fonts.ui, fontSize: 13.5, lineHeight: 19, color: colors.inkDim },
  });
};
