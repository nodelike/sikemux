import { useEffect, useMemo, useRef, useState } from 'react';
import {
  AccessibilityInfo,
  Keyboard,
  KeyboardAvoidingView,
  Platform,
  Pressable,
  StyleSheet,
  Text,
  View,
  type NativeScrollEvent,
  type NativeSyntheticEvent,
} from 'react-native';
import { useLocalSearchParams } from 'expo-router';
import { FlashList, type FlashListRef } from '@shopify/flash-list';

import { activityText, composerPlaceholder } from '@mac/chat/chatStatus';
import { sentPrompts } from '@mac/chat/promptHistory';
import { rowMeta } from '@mac/chat/messageMeta';
import { activeToolLabel } from '@mac/chat/toolLabels';
import { workFolds } from '@mac/chat/transcript';
import type { ChatMessage, ChatState } from '@mac/chat/types';
import { ChatMenu } from '@/chat/ChatMenu';
import { askTitle, Composer, RecentSheet } from '@/chat/Composer';
import { FoldsContext } from '@/chat/folds';
import { LiveSheet, LiveStrip } from '@/chat/Live';
import { hasLiveWork, liveKey, liveWork } from '@/chat/liveWork';
import { stripMeta, stripOwner } from '@/chat/messageMeta';
import { Activity, Earlier, Message, ProviderContext, Queued, WorkSummary } from '@/chat/Transcript';
import { useChat } from '@/chat/useChat';
import { retry as reconnect, useDevices, useLive, type Live } from '@/devices/hub';
import { dismissCardsFor } from '@/notify/cards';
import { deviceName } from '@/devices/paired';
import { chatTitle, providerName } from '@/devices/words';
import { haptics } from '@/ui/haptics';
import { AgentIcon, Icon } from '@/ui/Icon';
import { Button } from '@/ui/controls';
import { pauseBackdrop } from '@/ui/motion';
import { goBack } from '@/ui/navigate';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { Working } from '@/ui/status';
import { fonts, type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';

/** How near the end, as a share of the transcript's height, still counts as reading the latest. */
const FOLLOW = 0.1;

function capitalised(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1) + (text.endsWith('.') ? '' : '.');
}

function placeholderFor(state: ChatState, disconnected: boolean): string {
  if (disconnected) return 'Reconnect to continue this conversation';
  if (state.connection !== 'ready') return composerPlaceholder(state, { resuming: false, disconnected });
  if (state.running) return 'Send to queue behind the running turn';
  return state.commands.length ? 'Ask about this project, or type / for commands' : 'Ask about this project';
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
  const { session, chat } = useChat(core, agentId);
  const state = chat.agent;
  const request = chat.permissions[0];
  const liveId = state.running ? state.messages[state.messages.length - 1]?.id : undefined;
  const [open, setOpen] = useState<string>();
  const openStrip = useMemo(() => (open ? stripMeta(state.messages, open) : null), [open, state.messages]);
  const onTap = (message: ChatMessage) => {
    const owner = stripOwner(state.messages, message.id);
    setOpen((shown) => (shown === owner ? undefined : owner));
  };
  const folds = useMemo(() => workFolds(state.messages, state.running), [state.messages, state.running]);
  const [openWork, setOpenWork] = useState<ReadonlySet<string>>(() => new Set());
  const toggleWork = (id: string) =>
    setOpenWork((shown) => {
      const next = new Set(shown);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const marks = useMemo(
    () => [chat.replayed, chat.unsent, chat.sentFiles, liveId, open, openStrip, folds, openWork],
    [chat.replayed, chat.unsent, chat.sentFiles, liveId, open, openStrip, folds, openWork],
  );
  const fresh = useMemo(() => liveWork(state, chat.queued), [state, chat.queued]);
  // The strip redraws, and with it the composer, only when what it counts changes, not on every word streamed.
  const [work, setWork] = useState(fresh);
  if (liveKey(fresh) !== liveKey(work)) setWork(fresh);
  const sent = useMemo(
    () =>
      sentPrompts(
        state.messages,
        chat.queued.map((held) => held.text),
      ),
    [state.messages, chat.queued],
  );
  const [showLive, setShowLive] = useState(false);
  const [menu, setMenu] = useState(false);
  const [recent, setRecent] = useState(false);
  const openLive = () => setShowLive(true);
  const openRecent = () => setRecent(true);
  useEffect(() => dismissCardsFor(core, agentId), [core, agentId]);
  useArrivals(state.running, request?.requestId, {
    done: `${providerName(provider)} finished`,
    asks: request ? `${providerName(provider)} asks: ${askTitle(request)}` : '',
  });
  const list = useRef<FlashListRef<ChatMessage>>(null);
  const [away, setAway] = useState(false);
  const awayRef = useRef(false);
  const bottom = useBottomGap();
  const onScroll = ({ nativeEvent }: NativeSyntheticEvent<NativeScrollEvent>) => {
    const { contentOffset, contentSize, layoutMeasurement } = nativeEvent;
    const now = contentSize.height - contentOffset.y - layoutMeasurement.height > layoutMeasurement.height * FOLLOW;
    awayRef.current = now;
    setAway(now);
  };
  useEffect(() => {
    // The keyboard takes the bottom of the screen; someone reading the latest keeps reading it.
    const shown = Keyboard.addListener(Platform.OS === 'ios' ? 'keyboardWillShow' : 'keyboardDidShow', () => {
      if (!awayRef.current) requestAnimationFrame(() => list.current?.scrollToEnd({ animated: true }));
    });
    return () => shown.remove();
  }, []);
  const toEnd = () => requestAnimationFrame(() => list.current?.scrollToEnd({ animated: true }));
  const activity = activityText({ ...state, permissions: chat.permissions }, activeToolLabel(state.messages));
  const title = state.title ?? (info ? chatTitle(info) : providerName(provider));
  const watchOnly = access === 'watch';
  const steerable = state.capabilities.steering === true && state.running && !watchOnly;
  const liveShown = hasLiveWork(fresh) && chat.attached === 'live';
  const recalls = sent.length > 0 && !watchOnly;
  const strip = liveShown ? <LiveStrip work={work} provider={provider} onOpen={openLive} /> : null;
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
        title={
          <>
            <AgentIcon provider={provider} size={17} />
            <Text style={styles.title} numberOfLines={1} accessibilityRole="header">
              {title}
            </Text>
          </>
        }
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
          <ProviderContext value={provider}>
            <FoldsContext value={session.folds}>
              <View style={styles.transcript}>
                <FlashList
                  ref={list}
                  data={state.messages}
                  keyExtractor={(message) => message.id}
                  getItemType={(message) => message.role}
                  extraData={marks}
                  renderItem={({ item, index }) => {
                    const fold = folds.get(index);
                    const workId = fold ? state.messages[fold.start].id : '';
                    const folded = fold !== undefined && !openWork.has(workId);
                    const summary =
                      fold && index === fold.start ? (
                        <WorkSummary
                          took={rowMeta(state.messages, fold.end).took}
                          calls={fold.calls}
                          open={!folded}
                          onToggle={() => toggleWork(workId)}
                        />
                      ) : null;
                    if (fold && folded && index < fold.end) return summary ?? <View />;
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
                          from={fold && folded ? fold.from : 0}
                        />
                      </>
                    );
                  }}
                  ListHeaderComponent={
                    chat.hasEarlier ? <Earlier failed={chat.earlier === 'failed'} onRetry={session.loadEarlier} /> : null
                  }
                  onStartReached={chat.hasEarlier && chat.earlier === 'idle' ? session.loadEarlier : undefined}
                  ListFooterComponent={
                    <>
                      {chat.queued.length ? <Queued held={chat.queued} sentFiles={chat.sentFiles} /> : null}
                      {activity ? <Activity provider={provider} label={activity} since={chat.turnSince} /> : null}
                      {state.error ? <Text style={styles.error}>{state.error}</Text> : null}
                    </>
                  }
                  contentContainerStyle={styles.content}
                  maintainVisibleContentPosition={{
                    startRenderingFromBottom: true,
                    autoscrollToBottomThreshold: FOLLOW,
                    animateAutoScrollToBottom: false,
                  }}
                  onScroll={onScroll}
                  scrollEventThrottle={100}
                  keyboardDismissMode="interactive"
                />
                {away ? (
                  <Pressable
                    onPress={toEnd}
                    style={styles.jump}
                    hitSlop={8}
                    accessibilityRole="button"
                    accessibilityLabel="Jump to latest message">
                    <Icon name="IconArrowDown" size={16} color={colors.inkDim} />
                  </Pressable>
                ) : null}
              </View>
            </FoldsContext>
          </ProviderContext>
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
              placeholder={placeholderFor(state, live.status !== 'open')}
              permissionMode={chat.permissionMode ?? info?.permissionMode ?? ''}
              modeLocked={
                state.running || chat.permissions.length > 0 || chat.changingMode || state.connection !== 'ready' || live.status !== 'open'
              }
              watchOnly={watchOnly}
              offline={live.status !== 'open'}
              hostName={hostName}
              onSent={toEnd}
              attachments={chat.attachments}
              strip={strip}
              onRecent={recalls ? openRecent : undefined}
            />
          </>
        ) : state.messages.length ? (
          <View style={[styles.bottomStatus, { paddingBottom: bottom }]}>{status}</View>
        ) : null}
      </KeyboardAvoidingView>
      <LiveSheet
        visible={showLive && liveShown}
        onClose={() => setShowLive(false)}
        work={fresh}
        provider={provider}
        steerable={steerable}
        canAct={!watchOnly}
        stopping={chat.stopping}
        onStopTask={session.stopTask}
        onSteer={(ids) => {
          session.steer(ids);
          toEnd();
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
        agentName={providerName(provider)}
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
    title: { fontFamily: fonts.uiSemibold, fontSize: 16, letterSpacing: -0.25, color: colors.ink, flexShrink: 1 },
    more: { width: 40, height: 40, borderRadius: 20, alignItems: 'center', justifyContent: 'center' },
    transcript: { flex: 1, borderTopWidth: 1, borderTopColor: colors.border },
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
