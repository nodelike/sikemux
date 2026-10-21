import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { AccessibilityInfo, type TextInput, KeyboardAvoidingView, Pressable, StyleSheet, Text, View } from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import { FlashList, type FlashListRef } from '@shopify/flash-list';
import MaskedView from '@react-native-masked-view/masked-view';
import Svg, { Defs, LinearGradient, Rect, Stop } from 'react-native-svg';

import { activityText, composerPlaceholder } from '@mac/chat/chatStatus';
import { sentPrompts } from '@mac/chat/promptHistory';
import { activeToolLabel } from '@mac/chat/toolLabels';
import { outlivesFold, subagentsById, workFolds, type WorkFold } from '@mac/chat/transcript';
import type { ChatMessage } from '@mac/chat/types';
import { ChatMenu } from '@/chat/ChatMenu';
import { ChatTitle } from '@/chat/ChatTitle';
import { askTitle, Composer, RecentSheet } from '@/chat/Composer';
import { FoldsContext } from '@/chat/folds';
import { useTranscriptScroll } from '@/chat/transcriptScroll';
import { HostContext } from '@/chat/hostFiles';
import { LiveSheet, LiveStrip } from '@/chat/Live';
import { AnsweredQuestion, AssumedQuestion, OpenQuestion } from '@/chat/Question';
import { useQuestions } from '@/chat/questions';
import { hasLiveWork, liveKey, liveWork } from '@/chat/liveWork';
import { stripMeta, stripOwner } from '@/chat/messageMeta';
import { Activity, Earlier, EarlierLost, Message, ProviderContext, Queued, SubagentsContext, WorkSummary } from '@/chat/Transcript';
import { useChat } from '@/chat/useChat';
import { retry as reconnect, useDevices, useLive, type Live } from '@/devices/hub';
import { dismissCardsFor } from '@/notify/cards';
import { deviceName } from '@/devices/paired';
import { agentLabel } from '@mac/agents/catalog';
import { haptics } from '@/ui/haptics';
import { Icon } from '@/ui/Icon';
import { Button } from '@/ui/controls';
import { pauseBackdrop } from '@/ui/motion';
import { goBack } from '@/ui/navigate';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { Working } from '@/ui/status';
import { fonts, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';

function capitalised(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1) + (text.endsWith('.') ? '' : '.');
}

/** Above the composer while the host is out of reach: the chat stays, and what is sent waits. */
function Reconnecting({ live, hostName, onTry }: { live: Live; hostName: string; onTry: () => void }) {
  const styles = useStyles(makeStyles);
  const closed = live.status === 'closed';
  return (
    <View style={styles.reconnect} accessibilityLiveRegion="polite">
      {closed ? null : <Working />}
      <Text style={styles.reconnectText} numberOfLines={2}>
        {closed ? `Can't reach ${hostName}. ${capitalised(live.problem)}` : `Reconnecting to ${hostName}…`}
      </Text>
      <Pressable onPress={onTry} hitSlop={10} accessibilityRole="button" accessibilityLabel={`Try reaching ${hostName} now`}>
        <Text style={styles.reconnectTry}>Try now</Text>
      </Pressable>
    </View>
  );
}

/** Something this phone asked for that did not happen, said where the person is looking. */
function Notice({ text, onDismiss }: { text: string; onDismiss: () => void }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  return (
    <View style={styles.notice} accessibilityLiveRegion="polite">
      <Icon name="IconWarning" size={13} color={colors.danger} />
      <Text style={styles.noticeText}>{capitalised(text)}</Text>
      <Pressable onPress={onDismiss} hitSlop={10} accessibilityRole="button" accessibilityLabel="Dismiss">
        <Icon name="IconClose" size={13} color={colors.inkDim} />
      </Pressable>
    </View>
  );
}

/** A turn ending and a request arriving are felt and, with a screen reader, heard. */
function useArrivals(running: boolean, waiting: string | undefined, said: { done: string; asks: string }) {
  const was = useRef(running);
  const asked = useRef(waiting);
  useEffect(() => {
    if (was.current && !running) {
      haptics.success();
      AccessibilityInfo.announceForAccessibility(said.done);
    }
    was.current = running;
  }, [running, said.done]);
  // A streaming reply needs the JavaScript thread more than the moving backdrop does.
  useEffect(() => (running ? pauseBackdrop() : undefined), [running]);
  useEffect(() => {
    if (waiting && waiting !== asked.current) {
      haptics.warning();
      AccessibilityInfo.announceForAccessibility(said.asks);
    }
    asked.current = waiting;
  }, [waiting, said.asks]);
}

/** How far the transcript fades out under the header and above the composer. */
const FADE = 22;

function Fade({ id, from }: { id: string; from: 0 | 1 }) {
  return (
    <Svg width="100%" height={FADE}>
      <Defs>
        <LinearGradient id={id} x1="0" y1="0" x2="0" y2="1">
          <Stop offset="0" stopColor="#000" stopOpacity={from} />
          <Stop offset="1" stopColor="#000" stopOpacity={1 - from} />
        </LinearGradient>
      </Defs>
      <Rect x="0" y="0" width="100%" height="100%" fill={`url(#${id})`} />
    </Svg>
  );
}

/** The header and composer float over the backdrop, so the transcript fades into them rather than ending on a line. */
function EdgeFades() {
  return (
    <View style={{ flex: 1 }}>
      <Fade id="transcript-top" from={0} />
      <View style={{ flex: 1, backgroundColor: '#000' }} />
      <Fade id="transcript-bottom" from={1} />
    </View>
  );
}

export default function Chat() {
  const { core, agent } = useLocalSearchParams<{ core: string; agent: string }>();
  // Each chat starts with its own scroll and keyboard state.
  return <ChatScreen key={`${core}:${agent}`} core={core} agentId={agent} />;
}

function ChatScreen({ core, agentId }: { core: string; agentId: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const live = useLive(core);
  const { devices } = useDevices();
  const device = devices.find((paired) => paired.core === core);
  const access = device?.access ?? 'full';
  const hostName = device ? deviceName(device) : 'the host';
  const info = live.snapshot?.chats.find((chat) => chat.agentId === agentId);
  const provider = info?.provider ?? 'agent';
  const { session, chat, connection } = useChat(core, agentId);
  const state = chat.agent;
  const request = chat.permissions[0];
  const liveId = state.running ? state.messages[state.messages.length - 1]?.id : undefined;
  const [open, setOpen] = useState<string>();
  const openStrip = useMemo(() => (open ? stripMeta(state.messages, open) : null), [open, state.messages]);
  // The list redraws a row when what it is handed changes, so nothing it is handed changes with every word streamed.
  const messages = useRef(state.messages);
  useEffect(() => {
    messages.current = state.messages;
  }, [state.messages]);
  const onTap = (message: ChatMessage) => {
    const owner = stripOwner(messages.current, message.id);
    setOpen((shown) => (shown === owner ? undefined : owner));
  };
  const [foldsBefore, setFoldsBefore] = useState<Map<number, WorkFold>>();
  const folds = useMemo(() => workFolds(state.messages, state.running, foldsBefore), [state.messages, state.running, foldsBefore]);
  if (folds !== foldsBefore) setFoldsBefore(folds);
  // The list redraws its rows when a summary opens or closes, and only then, so it watches those folds alone.
  const openWork = useSyncExternalStore(session.folds.subscribe, () => session.folds.openUnder('work/'));
  const workOpen = (id: string) => session.folds.isOpen(`work/${id}`, false);
  const toggleWork = (id: string) => session.folds.set(`work/${id}`, !workOpen(id));
  const marks = useMemo(
    () => [chat.replayed, chat.unsent, chat.sentFiles, liveId, open, openStrip, folds, openWork],
    [chat.replayed, chat.unsent, chat.sentFiles, liveId, open, openStrip, folds, openWork],
  );
  const subagents = useMemo(() => subagentsById(state.messages), [state.messages]);
  const fresh = useMemo(() => liveWork(state, chat.queued), [state, chat.queued]);
  // The strip redraws, and with it the composer, only when what it counts changes, not on every word streamed.
  const [work, setWork] = useState(fresh);
  if (liveKey(fresh) !== liveKey(work)) setWork(fresh);
  const [recent, setRecent] = useState(false);
  const sent = useMemo(
    () =>
      recent
        ? sentPrompts(
            state.messages,
            chat.queued.map((held) => held.text),
          )
        : [],
    [recent, state.messages, chat.queued],
  );
  const [showLive, setShowLive] = useState(false);
  const [menu, setMenu] = useState(false);
  const openLive = () => setShowLive(true);
  const openRecent = () => setRecent(true);
  useEffect(() => dismissCardsFor(core, agentId), [core, agentId]);
  const name = info?.name ?? agentLabel(provider);
  useArrivals(state.running, request?.requestId, {
    done: `${name} finished`,
    asks: request ? `${name} asks: ${askTitle(request)}` : '',
  });
  const list = useRef<FlashListRef<ChatMessage>>(null);
  const bottom = useBottomGap();
  const scroll = useTranscriptScroll(list, {
    has: chat.hasEarlier,
    idle: chat.earlier === 'idle',
    load: session.loadEarlier,
  });
  const activity = activityText({ ...state, permissions: chat.permissions }, activeToolLabel(state.messages));
  const title = state.title ?? live.snapshot?.inbox.agents.find((shown) => shown.agentId === agentId)?.title ?? agentLabel(provider);
  const watchOnly = access === 'watch';
  const steerable = state.capabilities.steering === true && state.running && !watchOnly;
  const liveShown = hasLiveWork(fresh) && chat.attached === 'live';
  const recalls = !watchOnly && (chat.queued.length > 0 || state.messages.some((message) => message.role === 'user'));
  const strip = liveShown ? <LiveStrip work={work} provider={provider} onOpen={openLive} /> : null;
  const questions = useQuestions(connection, agentId, `${info?.activeAt}|${info?.unread}|${state.running}`, state.messages, session.report);
  const input = useRef<TextInput>(null);
  const composing = watchOnly ? null : questions.composing;
  const unreachable = chat.attached !== 'live' && live.status === 'closed';
  const status = unreachable ? (
    <View style={styles.missing}>
      <Text style={styles.gone}>{`Can't reach ${hostName}. ${capitalised(live.problem)}`}</Text>
      <Button title="Try again" onPress={() => reconnect(core)} />
    </View>
  ) : chat.attached === 'missing' ? (
    <View style={styles.missing}>
      <Text style={styles.gone}>{chat.problem ? capitalised(chat.problem) : 'This chat is no longer running on the host.'}</Text>
      <Button title="Try again" onPress={session.retry} />
    </View>
  ) : chat.attached === 'attaching' ? (
    <View style={styles.attaching} accessibilityLiveRegion="polite">
      <Working />
      <Text style={type.meta}>
        {live.status === 'connecting' ? `Reaching ${hostName}…` : info?.asleep ? 'Waking the chat…' : 'Opening the chat…'}
      </Text>
    </View>
  ) : null;

  return (
    <Screen>
      <Nav
        title={<ChatTitle agentId={agentId} group={info?.group} provider={provider} name={info?.name} title={title} />}
        end={
          <Pressable
            onPress={() => setMenu(true)}
            hitSlop={6}
            style={({ pressed }) => [styles.more, pressed && { backgroundColor: colors.active }]}
            accessibilityRole="button"
            accessibilityLabel="Chat actions">
            <Icon name="IconMoreVertical" size={16} color={colors.tertiary} />
          </Pressable>
        }
      />
      <KeyboardAvoidingView style={{ flex: 1 }} behavior="padding">
        {chat.attached !== 'live' && !state.messages.length ? (
          <View style={[styles.transcript, styles.alone, { paddingBottom: bottom }]}>{status}</View>
        ) : (
          <HostContext value={core}>
            <ProviderContext value={provider}>
              <FoldsContext value={session.folds}>
                <SubagentsContext value={subagents}>
                  <View style={styles.transcript}>
                    <MaskedView style={styles.transcript} maskElement={<EdgeFades />}>
                      <FlashList
                        ref={list}
                        showsVerticalScrollIndicator={false}
                        data={state.messages}
                        keyExtractor={(message) => message.id}
                        getItemType={(message, index) => {
                          const fold = folds.get(index);
                          // Folded work draws nothing, so it is never recycled into a whole answer.
                          return fold && index < fold.end && !workOpen(fold.id) && !message.parts.some(outlivesFold)
                            ? 'folded'
                            : message.role;
                        }}
                        extraData={marks}
                        renderItem={({ item, index }) => {
                          const fold = folds.get(index);
                          const workId = fold ? fold.id : '';
                          const folded = fold !== undefined && !workOpen(workId);
                          const summary =
                            fold && index === fold.start ? (
                              <WorkSummary
                                took={fold.took}
                                calls={fold.calls}
                                thought={fold.thought}
                                open={!folded}
                                onToggle={() => toggleWork(workId)}
                              />
                            ) : null;
                          if (fold && folded && index < fold.end && !item.parts.some(outlivesFold)) return summary ?? <View />;
                          return (
                            <>
                              {summary}
                              <Message
                                message={item}
                                live={item.id === liveId}
                                untimed={chat.replayed.has(item.id)}
                                unsent={chat.unsent.get(item.id)}
                                onRetry={session.retrySend}
                                onTap={onTap}
                                strip={item.id === open ? openStrip : undefined}
                                sentFiles={chat.sentFiles}
                                from={fold && folded ? (index < fold.end ? item.parts.length : fold.from) : 0}
                              />
                            </>
                          );
                        }}
                        ListHeaderComponent={
                          chat.hasEarlier ? (
                            <Earlier
                              failed={chat.earlier === 'failed'}
                              loading={chat.earlier === 'loading'}
                              onRetry={session.loadEarlier}
                            />
                          ) : state.earlierLost ? (
                            <EarlierLost />
                          ) : null
                        }
                        ListFooterComponent={
                          <>
                            {chat.queued.length ? <Queued held={chat.queued} sentFiles={chat.sentFiles} /> : null}
                            {activity ? <Activity provider={provider} label={activity} since={chat.turnSince} /> : null}
                            {state.error ? <Text style={styles.error}>{state.error}</Text> : null}
                            {questions.answered.map(({ question, answer }) => (
                              <AnsweredQuestion
                                key={question.id}
                                asker={question.name}
                                cli={question.cli}
                                question={question.asked}
                                answer={answer}
                                secret={question.secret}
                              />
                            ))}
                            {questions.open.map((question) => (
                              <OpenQuestion
                                key={question.id}
                                question={question}
                                canAnswer={!watchOnly}
                                busy={questions.busy === question.id}
                                writing={questions.writing && composing?.id === question.id}
                                onChoose={(choice) => void questions.answer(question, { choice }, question.choices[choice]?.label ?? '')}
                                onWrite={() => {
                                  questions.write(question);
                                  input.current?.focus();
                                }}
                                onSecret={(text) => questions.answer(question, { answer: text }, '')}
                              />
                            ))}
                            {questions.assumed.map((question) => (
                              <AssumedQuestion
                                key={question.id}
                                question={question}
                                canAnswer={!watchOnly}
                                busy={questions.busy === question.id}
                                onFine={() => void questions.answer(question, { fine: true }, null)}
                                onChange={() => {
                                  questions.write(question);
                                  input.current?.focus();
                                }}
                              />
                            ))}
                          </>
                        }
                        contentContainerStyle={styles.content}
                        {...scroll.listProps}
                        keyboardDismissMode="interactive"
                      />
                    </MaskedView>
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
                </SubagentsContext>
              </FoldsContext>
            </ProviderContext>
          </HostContext>
        )}
        {chat.attached === 'live' ? (
          <>
            {live.status !== 'open' ? <Reconnecting live={live} hostName={hostName} onTry={() => reconnect(core)} /> : null}
            {chat.notice ? <Notice text={chat.notice} onDismiss={session.dismissNotice} /> : null}
            <Composer
              session={session}
              provider={provider}
              running={state.running}
              setup={state.setup}
              usage={state.usage}
              commands={state.commands}
              requests={chat.permissions}
              answering={chat.answering}
              placeholder={
                composing
                  ? `Answer ${composing.name}`
                  : composerPlaceholder(state, {
                      resuming: false,
                      disconnected: live.status !== 'open',
                      commands: state.commands.length > 0,
                    })
              }
              permissionMode={chat.permissionMode ?? ''}
              modeLocked={
                state.running || chat.permissions.length > 0 || chat.changingMode || state.connection !== 'ready' || live.status !== 'open'
              }
              watchOnly={watchOnly}
              offline={live.status !== 'open'}
              hostName={hostName}
              onSent={scroll.toEnd}
              attachments={chat.attachments}
              strip={strip}
              onRecent={recalls ? openRecent : undefined}
              inputRef={input}
              question={
                composing && {
                  name: composing.name,
                  cli: composing.cli,
                  text: composing.text,
                  onRemove: questions.stopComposing,
                  onAnswer: (text) => questions.answer(composing, { answer: text }, text),
                }
              }
            />
          </>
        ) : state.messages.length ? (
          <View style={[styles.bottomStatus, { paddingBottom: bottom }]}>{status}</View>
        ) : null}
      </KeyboardAvoidingView>
      <LiveSheet
        visible={showLive && liveShown}
        onClose={() => setShowLive(false)}
        work={showLive ? fresh : work}
        provider={provider}
        steerable={steerable}
        canAct={!watchOnly}
        stopping={chat.stopping}
        onStopTask={session.stopTask}
        onSteer={(ids) => {
          session.steer(ids);
          scroll.toEnd();
        }}
        onDrop={session.dropQueued}
      />
      <RecentSheet
        visible={recent}
        prompts={sent}
        onClose={() => setRecent(false)}
        onPick={(text) => {
          session.setDraft(text);
          setRecent(false);
        }}
      />
      <ChatMenu
        visible={menu}
        onClose={() => setMenu(false)}
        core={core}
        agentId={agentId}
        agentName={name}
        hostName={hostName}
        onRecent={recalls ? openRecent : undefined}
        onStop={
          watchOnly || chat.attached !== 'live'
            ? undefined
            : async () => {
                const stopped = await session.stopChat();
                if (stopped) goBack();
                return stopped;
              }
        }
      />
    </Screen>
  );
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    more: { width: 40, height: 40, borderRadius: 20, alignItems: 'center', justifyContent: 'center' },
    transcript: { flex: 1 },
    alone: { justifyContent: 'flex-end' },
    content: { paddingHorizontal: 18, paddingTop: 6, paddingBottom: 28 },
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
      shadowColor: '#000',
      shadowOpacity: 0.4,
      shadowRadius: 12,
      shadowOffset: { width: 0, height: 6 },
      elevation: 6,
    },
    attaching: { flexDirection: 'row', alignItems: 'center', gap: 10, alignSelf: 'center', paddingVertical: 24 },
    missing: { gap: 16, paddingHorizontal: 18, paddingVertical: 24 },
    bottomStatus: { borderTopWidth: 1, borderTopColor: colors.border },
    gone: { ...type.meta, textAlign: 'center' },
    error: { fontFamily: fonts.ui, fontSize: 13.5, color: colors.danger, marginTop: 10 },
    reconnect: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 10,
      marginHorizontal: 10,
      marginTop: 8,
      paddingVertical: 9,
      paddingHorizontal: 12,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
    },
    reconnectText: { flex: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.secondary },
    reconnectTry: { fontFamily: fonts.uiSemibold, fontSize: 13, color: colors.ink },
    notice: {
      flexDirection: 'row',
      alignItems: 'center',
      gap: 9,
      marginHorizontal: 10,
      marginTop: 8,
      paddingVertical: 9,
      paddingHorizontal: 12,
      borderRadius: 12,
      borderWidth: 1,
      borderColor: colors.border,
      backgroundColor: colors.overlay,
    },
    noticeText: { flex: 1, fontFamily: fonts.ui, fontSize: 13, color: colors.secondary },
  });
};
