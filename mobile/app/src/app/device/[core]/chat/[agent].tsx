import { useChatFind, useRevealFound } from '@/chat/useChatFind';
import { takeFind } from '@/devices/pendingFind';
import { useEditingRow } from '@/chat/useEditingRow';
import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { type TextInput, Pressable, StyleSheet, Text, View } from 'react-native';
import { router, useLocalSearchParams } from 'expo-router';
import { FlashList, type FlashListRef } from '@shopify/flash-list';
import MaskedView from '@react-native-masked-view/masked-view';
import { KeyboardStickyView } from 'react-native-keyboard-controller';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { activityDoing, composerPlaceholder } from '@shared/chat/chatStatus';
import { forgetShown } from '@/chat/LeftOut';
import { sentPrompts } from '@/chat/promptHistory';
import type { ChatMessage, PersonCall } from '@shared/wire.generated';
import { ChatMenu } from '@/chat/ChatMenu';
import { AwaySheet } from '@/devices/AwaySheet';
import { FindBar } from '@/chat/FindBar';
import { FindingContext } from '@/chat/found';
import { ChatTitle } from '@/chat/ChatTitle';
import { Composer } from '@/chat/Composer';
import { askTitle } from '@/chat/PermissionDock';
import { RecentSheet } from '@/chat/RecentSheet';
import { FoldsContext, rowFold, runsOf, shownRows } from '@/chat/folds';
import { useTranscriptScroll } from '@/chat/transcriptScroll';
import { HostContext } from '@/chat/hostFiles';
import { WorktreeNote, worktreeOf } from '@/chat/Worktree';
import { LiveSheet, LiveStrip } from '@/chat/Live';
import { AssumedQuestion } from '@/chat/Question';
import { useAssumptions } from '@/chat/assumptions';
import { CallDock } from '@/calls/CallDock';
import { TurnLine } from '@/calls/TurnLine';
import { answerWritten, composingCall, queueOf, stopComposing, useKept, waitingCall } from '@/calls/hostCalls';
import { NO_CHIPS, TELL_CHIEF, answering as answeringWords, sendToTold, typeYourAnswer } from '@shared/calls/words';
import { AwayCard } from '@/calls/AwayCard';
import { useOnScreen } from '@/calls/onScreen';
import { hasLiveWork, liveKey, liveWork } from '@/chat/liveWork';
import { stripOf, stripOwner } from '@/chat/messageMeta';
import { ToolDetailContext } from '@/chat/Tools';
import { Trouble } from '@/chat/Trouble';
import { Message, plainlySent, TeamFold, WorkSummary } from '@/chat/Transcript';
import { Activity } from '@/chat/Activity';
import { SmartPick } from '@/chat/Notices';
import { Earlier, EarlierLost, Queued } from '@/chat/TranscriptEdges';
import { ActsContext, AgentContext, liveRunKey, ProviderContext, SubagentsContext } from '@/chat/transcriptContext';
import { useChat } from '@/chat/useChat';
import { useHostCall } from '@/core/hostCalls';
import { type Translate, useT } from '@/i18n';
import { retry as reconnect, useDevices, useLive } from '@/devices/hub';
import { dismissCardsFor } from '@/notify/cards';
import { deviceName } from '@/devices/paired';
import { agentLabel } from '@shared/agents/catalog';
import { subagentsOf } from '@shared/chat/subagents';
import { Icon } from '@/ui/Icon';
import { Button } from '@/ui/controls';
import { goBack } from '@/ui/navigate';
import { Nav, Screen, useBottomGap } from '@/ui/screen';
import { Working } from '@/devices/AgentMarks';
import { type Palette, typeFor, useColors, useStyles, useType } from '@/ui/theme';
import { EdgeFades, Notice, Reconnecting, capitalised, renderScrollComponent, useArrivals } from '@/chat/ChatChrome';

export default function Chat() {
  const { core, agent } = useLocalSearchParams<{ core: string; agent: string }>();
  // Each chat starts with its own scroll and keyboard state.
  return <ChatScreen key={`${core}:${agent}`} core={core} agentId={agent} />;
}

function ChatScreen({ core, agentId }: { core: string; agentId: string }) {
  const colors = useColors();
  const styles = useStyles(makeStyles);
  const type = useType();
  const t = useT();
  const live = useLive(core);
  const { devices } = useDevices();
  const device = devices.find((paired) => paired.core === core);
  const access = device?.access ?? 'full';
  const hostName = device ? deviceName(device, t) : t('the host');
  const info = live.snapshot?.chats.find((chat) => chat.agentId === agentId);
  const provider = info?.provider ?? 'agent';
  const { session, chat, connection } = useChat(core, agentId);
  useEffect(() => () => forgetShown(agentId), [agentId]);
  const state = chat.view;
  const request = chat.permissions[0];
  const liveId = state.running ? state.messages[state.messages.length - 1]?.id : undefined;
  const [open, setOpen] = useState<string>();
  const openStrip = useMemo(() => (open ? stripOf(state.messages, open) : null), [open, state.messages]);
  // The list redraws a row when what it is handed changes, so nothing it is handed changes with every word streamed.
  const messages = useRef(state.messages);
  useEffect(() => {
    messages.current = state.messages;
  }, [state.messages]);
  const onTap = (message: ChatMessage) => {
    const owner = stripOwner(messages.current, message.id);
    setOpen((shown) => (shown === owner ? undefined : owner));
  };
  const [editing, setEditing] = useState<string>();
  // The list redraws its rows when a summary opens or closes, and only then, so it watches those folds alone.
  const openWork = useSyncExternalStore(session.folds.subscribe, () => session.folds.openUnder('work/') + session.folds.openUnder('team'));
  const workOpen = (id: string) => session.folds.isOpen(`work/${id}`, false);
  const toggleWork = (id: string) => session.folds.set(`work/${id}`, !workOpen(id));
  const { rows, runs } = useMemo(() => {
    const opened = new Set(openWork.split('\n'));
    const runs = runsOf(state.messages, (key) => opened.has(key));
    return { rows: shownRows(state.messages, (id) => opened.has(`work/${id}`), runs), runs };
  }, [state.messages, openWork]);
  const toggleRun = (id: string) => {
    scroll.hold();
    session.folds.set(`team/${id}`, !session.folds.isOpen(`team/${id}`, false));
    session.folds.set(`team-all/${id}`, false);
  };
  const showRunEarlier = (id: string) => {
    scroll.hold();
    session.folds.set(`team-all/${id}`, true);
  };
  const { find, finding, shown, shownIndex, openFind, closeFind } = useChatFind(session, rows, workOpen, () => setEditing(undefined));
  // A chat opened from a search of what was said opens its find on those words once it is attached.
  const foundWords = useRef<string>(undefined);
  useEffect(() => {
    if (chat.attached !== 'live') return;
    const words = takeFind(core, agentId);
    if (!words) return;
    foundWords.current = words;
    openFind();
  }, [chat.attached, core, agentId, openFind]);
  useEffect(() => {
    if (!find || !foundWords.current) return;
    find.setText(foundWords.current);
    foundWords.current = undefined;
  }, [find]);
  const watchOnly = access === 'watch';
  useOnScreen(core, agentId, access === 'full', chat.attached === 'live', connection);
  const canEdit = !watchOnly && state.capabilities.editing && state.connection === 'ready' && !state.running && chat.attached === 'live';
  if (editing !== undefined && !canEdit) setEditing(undefined);
  const editable = (message: ChatMessage) =>
    canEdit && message.role === 'user' && message.promptId !== undefined && !message.context?.length && !chat.unsent.has(message.id);
  const marks = useMemo(
    () => [chat.unsent, chat.sentFiles, liveId, open, openStrip, openWork, canEdit, editing],
    [chat.unsent, chat.sentFiles, liveId, open, openStrip, openWork, canEdit, editing],
  );
  const subagents = useMemo(() => subagentsOf(state.messages, state.subagentRows), [state.messages, state.subagentRows]);
  const { handoff, guests, plan, runningSubagents, tasks, queued } = state;
  const fresh = useMemo(
    () => liveWork({ handoff, guests, plan, runningSubagents, tasks, queued }),
    [handoff, guests, plan, runningSubagents, tasks, queued],
  );
  // The strip redraws, and with it the composer, only when what it counts changes, not on every word streamed.
  const [work, setWork] = useState(fresh);
  if (liveKey(fresh) !== liveKey(work)) setWork(fresh);
  const [recent, setRecent] = useState(false);
  const sent = useMemo(
    () =>
      recent
        ? sentPrompts(
            state.messages,
            queued.map((held) => held.text),
          )
        : [],
    [recent, state.messages, queued],
  );
  const [showLive, setShowLive] = useState(false);
  const [menu, setMenu] = useState(false);
  const [away, setAway] = useState(false);
  const openLive = () => setShowLive(true);
  const openRecent = () => setRecent(true);
  useEffect(() => dismissCardsFor(core, agentId), [core, agentId]);
  const name = info?.name ?? agentLabel(provider);
  useArrivals(state.running, request?.requestId, {
    done: t('{name} finished', { name }),
    asks: request ? t('{name} asks: {question}', { name, question: askTitle(request, t) }) : '',
  });
  const list = useRef<FlashListRef<ChatMessage>>(null);
  const bottom = useBottomGap();
  const homeBar = useSafeAreaInsets().bottom;
  const scroll = useTranscriptScroll(
    list,
    {
      has: chat.hasEarlier,
      idle: chat.earlier === 'idle',
      load: session.loadEarlier,
    },
    editing !== undefined || find !== null,
  );
  useRevealFound(scroll, shown, shownIndex);
  const startEdit = (message: ChatMessage) => {
    setOpen(undefined);
    setEditing(message.id);
  };
  const editingRow = useEditingRow(editing, setEditing, session, scroll, rows);
  const doing = chat.stoppingTurn ? { verb: t('Stopping') } : chat.permissions.length > 0 ? null : activityDoing(state.activity, t);
  const liveRun = liveRunKey(state.messages, state.activeTool);
  // While the run holding the running call is open, its line already names what runs.
  const runShown = useSyncExternalStore(session.folds.subscribe, () => liveRun !== null && session.folds.isOpen(liveRun, true));
  const activity = doing && runShown ? { verb: doing.verb } : doing;
  const listed = live.snapshot?.inbox.agents.find((shown) => shown.agentId === agentId);
  const title = state.title ?? listed?.title ?? agentLabel(provider);
  const steerable = state.capabilities.steering === true && state.running && !watchOnly;
  // The person may change how sending mid-turn goes between turns, so each turn reads it afresh.
  const preferences = useHostCall(connection, 'preferences', {}, String(state.running)).data;
  const steers = steerable && preferences?.midTurnSend !== 'queue';
  const saved = live.snapshot?.workspace.agents?.find((shown) => shown.id === agentId);
  const providerOf = (other: string) => live.snapshot?.workspace.agents?.find((shown) => shown.id === other)?.provider ?? 'agent';
  const worktree = worktreeOf(saved?.worktree);
  const project = live.snapshot?.workspace.projects.find((known) => known.id === saved?.project);
  const accounts = live.snapshot?.workspace.launchers.filter((known) => known.provider === provider) ?? [];
  const place = {
    host: devices.length > 1 ? hostName : undefined,
    project: project?.name,
    branch: worktree?.branch || project?.git?.branch,
    account:
      accounts.length > 1 && saved?.profileId ? accounts.find((known) => known.id.endsWith(`:${saved.profileId}`))?.label : undefined,
  };
  const startOver = saved
    ? () => {
        const launcher = live.snapshot?.workspace.launchers.find(
          (known) => known.provider === saved.provider && (!saved.profileId || known.id.endsWith(`:${saved.profileId}`)),
        );
        const params = { core, project: saved.project, ...(launcher ? { launcher: launcher.id } : {}) };
        router.push({ pathname: '/device/[core]/new', params });
      }
    : undefined;
  const liveShown = hasLiveWork(fresh) && chat.attached === 'live';
  const recalls = !watchOnly && (queued.length > 0 || state.messages.some((message) => message.role === 'user'));
  const strip = liveShown ? <LiveStrip work={work} provider={provider} onOpen={openLive} /> : null;
  const assumptions = useAssumptions(
    connection,
    live.snapshot?.calls,
    agentId,
    `${info?.activeAt}|${info?.unread}|${state.running}`,
    session.report,
  );
  const input = useRef<TextInput>(null);
  const keptCalls = useKept(core);
  // The hub keeps the host's calls as the same array while none changes, so typing beside a docked call does not
  // redraw it on every view the host sends.
  const calls = live.snapshot?.calls;
  // While the chief's chat shows what happened while the person was away, it docks every call that card counts.
  const awayCalls = state.chief ? state.away?.calls : undefined;
  const queue = useMemo(() => queueOf(calls, agentId, awayCalls), [calls, agentId, awayCalls]);
  const docked = waitingCall(queue, keptCalls, agentId);
  // Change on an assumption wins over a call docked in the same chat, until it is sent or taken off.
  const changing = watchOnly ? null : assumptions.changing;
  // In the chief's chat, typed words answer a docked call only when it is the chief's own and the only one open.
  const answersHere = !state.chief || (!!docked?.asker.chief && queue.length === 1);
  const composing = watchOnly || changing || !answersHere ? undefined : answeringIn(composingCall(docked, keptCalls), t);
  const canAnswerCall = !watchOnly && live.status === 'open';
  // The composer says who an idle chat's words go to, as the Mac does: the chief, or a worker the chief hears.
  const composerTo = state.chief
    ? { words: t(TELL_CHIEF), commands: true }
    : state.chiefHears && info?.name
      ? { words: sendToTold(info.name, t), commands: false }
      : undefined;
  // React's compiler keeps this element while its inputs stay the same, so a view from the host leaves the card be.
  const dock = docked ? (
    <CallDock
      key={docked.id}
      core={core}
      calls={calls}
      call={docked}
      chat={agentId}
      queue={queue}
      connection={connection}
      canAnswer={canAnswerCall}
      report={session.report}
    />
  ) : null;
  const unreachable = chat.attached !== 'live' && live.status === 'closed';
  const status = unreachable ? (
    <View style={styles.missing}>
      <Text style={styles.gone}>{`${t("Can't reach {host}.", { host: hostName })} ${capitalised(live.problem)}`}</Text>
      <Button title={t('Try again')} onPress={() => reconnect(core)} />
    </View>
  ) : chat.attached === 'missing' ? (
    <View style={styles.missing}>
      <Text style={styles.gone}>{chat.problem ? capitalised(chat.problem) : t('This chat is no longer running on the host.')}</Text>
      <Button title={t('Try again')} onPress={session.retry} />
    </View>
  ) : chat.attached === 'attaching' ? (
    <View style={styles.attaching} accessibilityLiveRegion="polite">
      <Working />
      <Text style={type.meta}>
        {live.status === 'connecting'
          ? t('Reaching {host}…', { host: hostName })
          : info?.state === 'asleep'
            ? t('Waking the chat…')
            : t('Opening the chat…')}
      </Text>
    </View>
  ) : null;

  const screen = (
    <Screen>
      <Nav
        title={
          <ChatTitle agentId={agentId} turn={state.turn?.state} provider={provider} name={info?.name} title={title} chief={state.chief} />
        }
        end={
          <Pressable
            onPress={() => setMenu(true)}
            hitSlop={6}
            style={({ pressed }) => [styles.more, pressed && { backgroundColor: colors.active }]}
            accessibilityRole="button"
            accessibilityLabel={t('Chat actions')}>
            <Icon name="IconMoreVertical" size={16} color={colors.tertiary} />
          </Pressable>
        }
      />
      <View style={styles.transcript}>
        {chat.attached !== 'live' && !state.messages.length ? (
          <View style={[styles.transcript, styles.alone, { paddingBottom: bottom }]}>{status}</View>
        ) : (
          <HostContext value={core}>
            <ProviderContext value={provider}>
              <AgentContext value={agentId}>
                <ActsContext value={access === 'full'}>
                  <FoldsContext value={session.folds}>
                    <SubagentsContext value={subagents}>
                      <ToolDetailContext value={session.toolDetail}>
                        <View style={styles.transcript}>
                          <MaskedView style={styles.transcript} maskElement={<EdgeFades />}>
                            <FlashList
                              ref={list}
                              showsVerticalScrollIndicator={false}
                              data={rows}
                              keyExtractor={(message) => message.id}
                              // Folded work draws nothing, so it is never recycled into a whole answer.
                              getItemType={(message) => (rowFold(message, workOpen, runs.get(message.id)).hidden ? 'folded' : message.role)}
                              extraData={marks}
                              renderItem={({ item, index }) => {
                                const run = runs.get(item.id);
                                const { summary: fold, hidden, shown: kept } = rowFold(item, workOpen, run);
                                const head = run?.head ? (
                                  <TeamFold
                                    line={run.head}
                                    open={session.folds.isOpen(`team/${run.id}`, false)}
                                    all={session.folds.isOpen(`team-all/${run.id}`, false)}
                                    onToggle={() => toggleRun(run.id)}
                                    onAll={() => showRunEarlier(run.id)}
                                  />
                                ) : null;
                                const note =
                                  index === 0 && worktree && !chat.hasEarlier ? (
                                    <WorktreeNote branch={worktree.branch} path={worktree.path} />
                                  ) : null;
                                const summary = fold ? (
                                  <WorkSummary
                                    took={fold.took}
                                    calls={fold.calls}
                                    thought={fold.thought}
                                    open={workOpen(fold.id)}
                                    onToggle={() => toggleWork(fold.id)}
                                  />
                                ) : null;
                                if (hidden)
                                  return note || summary || head ? (
                                    <>
                                      {note}
                                      {head}
                                      {summary}
                                    </>
                                  ) : (
                                    <View />
                                  );
                                return (
                                  <>
                                    {note}
                                    {head}
                                    {summary}
                                    <Message
                                      message={item}
                                      live={item.id === liveId}
                                      unsent={chat.unsent.get(item.id)}
                                      onRetry={session.retrySend}
                                      onTap={onTap}
                                      strip={item.id === open ? openStrip : undefined}
                                      sentFiles={chat.sentFiles}
                                      kept={kept}
                                      onEdit={editable(item) ? startEdit : undefined}
                                      editing={item.id === editing ? editingRow : undefined}
                                      followsOwn={plainlySent(item) && plainlySent(rows[index - 1])}
                                      followedByOwn={plainlySent(item) && plainlySent(rows[index + 1])}
                                    />
                                  </>
                                );
                              }}
                              ListEmptyComponent={state.placed ? <SmartPick move={state.placed} provider={provider} /> : null}
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
                                  {queued.length ? <Queued held={queued} sentFiles={chat.sentFiles} /> : null}
                                  {state.away ? (
                                    <AwayCard
                                      away={state.away}
                                      connection={connection}
                                      full={access === 'full'}
                                      providerOf={providerOf}
                                      onOpen={(other) => router.push(`/device/${core}/chat/${other}`)}
                                    />
                                  ) : null}
                                  {activity ? (
                                    <Activity provider={provider} doing={activity} since={chat.turnSince} />
                                  ) : state.turn ? (
                                    <TurnLine
                                      agentId={agentId}
                                      provider={provider}
                                      turn={state.turn}
                                      connection={connection}
                                      canAct={!watchOnly && chat.attached === 'live' && live.status === 'open'}
                                      docked={!!docked}
                                      onLook={() => input.current?.focus()}
                                    />
                                  ) : null}
                                  <Trouble
                                    state={state}
                                    provider={provider}
                                    hostName={hostName}
                                    connection={connection}
                                    session={session}
                                    canAct={!watchOnly && chat.attached === 'live' && live.status === 'open'}
                                    onStartOver={startOver}
                                  />
                                  {assumptions.assumed.map((question) => (
                                    <AssumedQuestion
                                      key={question.id}
                                      question={question}
                                      canAnswer={!watchOnly}
                                      onFine={() => assumptions.fine(question)}
                                      onChange={() => {
                                        assumptions.change(question);
                                        input.current?.focus();
                                      }}
                                    />
                                  ))}
                                </>
                              }
                              contentContainerStyle={styles.content}
                              {...scroll.listProps}
                              renderScrollComponent={renderScrollComponent}
                              keyboardDismissMode="interactive"
                              keyboardShouldPersistTaps={editing === undefined ? 'never' : 'handled'}
                            />
                          </MaskedView>
                          {scroll.away ? (
                            <KeyboardStickyView style={styles.jumpPlace} offset={{ opened: homeBar }}>
                              <Pressable
                                onPress={scroll.toEnd}
                                style={styles.jump}
                                hitSlop={8}
                                accessibilityRole="button"
                                accessibilityLabel={t('Jump to latest message')}>
                                <Icon name="IconArrowDown" size={16} color={colors.inkDim} />
                              </Pressable>
                            </KeyboardStickyView>
                          ) : null}
                        </View>
                      </ToolDetailContext>
                    </SubagentsContext>
                  </FoldsContext>
                </ActsContext>
              </AgentContext>
            </ProviderContext>
          </HostContext>
        )}
        <KeyboardStickyView offset={{ opened: homeBar }}>
          {chat.attached === 'live' ? (
            <HostContext value={core}>
              {live.status !== 'open' ? <Reconnecting live={live} hostName={hostName} onTry={() => reconnect(core)} /> : null}
              {chat.notice ? <Notice text={chat.notice} onDismiss={session.dismissNotice} /> : null}
              {find ? (
                <FindBar find={find} onClose={closeFind} />
              ) : (
                <Composer
                  session={session}
                  core={core}
                  provider={provider}
                  running={state.running && !chat.stoppingTurn}
                  steers={steers}
                  model={state.model}
                  effort={state.effort}
                  usage={state.usage}
                  expiredCache={chat.expiredCache}
                  commands={state.commands}
                  requests={chat.permissions}
                  answering={chat.answering}
                  placeholder={
                    changing
                      ? typeYourAnswer(changing.name, t)
                      : composing
                        ? typeYourAnswer(composing.asker, t)
                        : composerPlaceholder(
                            state,
                            {
                              resuming: false,
                              disconnected: live.status !== 'open',
                              commands: state.commands.length > 0,
                              steers,
                              to: composerTo,
                            },
                            t,
                          )
                  }
                  permissionMode={chat.permissionMode ?? ''}
                  modeLocked={state.running || chat.permissions.length > 0 || state.connection !== 'ready' || live.status !== 'open'}
                  watchOnly={watchOnly}
                  offline={live.status !== 'open'}
                  hostName={hostName}
                  onSent={scroll.toEnd}
                  attachments={chat.attachments}
                  chips={chat.chips}
                  held={dock}
                  strip={strip}
                  onRecent={recalls ? openRecent : undefined}
                  inputRef={input}
                  place={place}
                  question={
                    changing
                      ? {
                          chip: answeringWords(changing.name, changing.text, t),
                          cli: changing.cli,
                          onRemove: assumptions.stopChanging,
                          onAnswer: (written) => assumptions.answer(changing, written),
                        }
                      : composing
                        ? {
                            chip: composing.chip,
                            cli: composing.cli,
                            chipsRefused: t(NO_CHIPS),
                            onRemove: () => stopComposing(core, composing.id),
                            onAnswer: (written) =>
                              answerWritten(
                                connection,
                                composing.id,
                                { answer: written.answer, paths: written.paths ?? [] },
                                session.report,
                              ),
                          }
                        : null
                  }
                />
              )}
            </HostContext>
          ) : state.messages.length ? (
            <View style={[styles.bottomStatus, { paddingBottom: bottom }]}>{status}</View>
          ) : null}
        </KeyboardStickyView>
      </View>
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
        onSendGuest={session.sendGuest}
        onDismissGuest={session.dismissGuest}
        onLeaveOutHandoff={session.dropHandoff}
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
        onFind={chat.attached === 'live' ? openFind : undefined}
        onRecent={recalls ? openRecent : undefined}
        onAway={watchOnly || !listed ? undefined : () => setAway(true)}
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
      <AwaySheet core={core} shown={away ? listed : undefined} onClose={() => setAway(false)} onDone={goBack} />
    </Screen>
  );
  return <FindingContext value={finding}>{screen}</FindingContext>;
}

const makeStyles = (colors: Palette) => {
  const type = typeFor(colors);
  return StyleSheet.create({
    more: { width: 40, height: 40, borderRadius: 20, alignItems: 'center', justifyContent: 'center' },
    transcript: { flex: 1 },
    alone: { justifyContent: 'flex-end' },
    content: { paddingHorizontal: 18, paddingTop: 6, paddingBottom: 28 },
    jumpPlace: { position: 'absolute', bottom: 8, alignSelf: 'center' },
    jump: {
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
  });
};

/** What the composer says while it answers `call`, worked out apart from the call so the docked card keeps its element. */
function answeringIn(call: PersonCall | undefined, t: Translate) {
  return (
    call && {
      id: call.id,
      cli: call.asker.cli,
      chip: answeringWords(call.asker.name, call.question, t),
      asker: call.asker.name,
    }
  );
}
