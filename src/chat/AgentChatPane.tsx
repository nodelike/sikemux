import { lazy, memo, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState } from "react";
import { errorText } from "../lib/format";
import { acpApi } from "../api/acp";
import { effortConfig, sessionConfigs, type SessionConfig } from "./sessionConfig";
import type { Agent, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { hasPrimaryModifier } from "../lib/platform";
import { IconArrowDown, IconFile, IconPlug, IconWarning } from "../ui/Icons";
import { chatReducer, initialChatState } from "./reducer";
import { EarlierTurns } from "./EarlierTurns";
import type { ChatMessage, ChatState } from "./types";
import { PathRootsProvider } from "./FileRef";
import { ChatWelcome } from "./ChatWelcome";
import { newFoldMemory } from "./longText";
import { sentPrompts } from "./promptHistory";
import { activeToolLabel } from "./toolLabels";
import { runningSubagents, subagentsById } from "./transcript";
import { activityText, composerPlaceholder as placeholderFor, connectingLabel, knownEffort, lostConnectionText } from "./chatStatus";
import { ChatAgentContext, ChatSubagentsContext, ReaderScrollContext } from "./chatAgent";
import { warmTranscript } from "./ChatMessageRow";
import { ChatTranscript } from "./ChatTranscript";
import { ChatComposer } from "./ChatComposer";
import { ComposerBuddy } from "../buddies/Buddy";
import { useMessageArrival } from "./useMessageArrival";
import { useAcpSession } from "./useAcpSession";
import { usePromptQueue } from "./usePromptQueue";
import { putDraft } from "./draftHandoff";
import { useChatWorktree } from "./useChatWorktree";
import { useStickToBottom } from "./useStickToBottom";
import { mentionContext, useMates } from "./agentMentions";
import { deliverToAgent } from "../agents/agentInbox";
import type { AgentQuestion } from "../agents/agentQuestions";
import type { OutgoingMessage } from "./queuedMessages";
import { answerText } from "./answerText";
import { useChatQuestions } from "./useChatQuestions";
import { ResumeActions } from "./ResumeActions";
import { ChatFailureActions, NetworkFailureActions, type SignInMethod } from "./ChatFailureActions";
import LiveStack from "./LiveStack";
import QuestionRows from "./QuestionRows";
import { QuestionDock, questionTitle } from "./QuestionDock";

/* Reading a whole transcript is one round trip to the parser, so this waits
   at most this long before showing it anyway. */
const READ_WAIT_MS = 1000;

function useReadAfterReplay(replaying: boolean, messages: readonly ChatMessage[]): boolean {
    const [reading, setReading] = useState(false);
    const wasReplayingRef = useRef(replaying);
    const messagesRef = useRef(messages);
    messagesRef.current = messages;
    useLayoutEffect(() => {
        const ended = wasReplayingRef.current && !replaying;
        wasReplayingRef.current = replaying;
        if (!ended) return;
        setReading(true);
        let live = true;
        const done = () => {
            if (live) setReading(false);
        };
        void warmTranscript(messagesRef.current).then(done, done);
        const timer = window.setTimeout(done, READ_WAIT_MS);
        return () => {
            live = false;
            window.clearTimeout(timer);
        };
    }, [replaying]);
    return reading;
}

function heldTranscript(shown: ChatState, next: ChatState): ChatState {
    return { ...next, messages: shown.messages, revision: shown.revision };
}

const ChatFind = lazy(() => import("./ChatFind"));
const WorktreeNote = lazy(() => import("./ChatWorktree").then(({ WorktreeNote }) => ({ default: WorktreeNote })));
const ProjectStrip = lazy(() => import("./ProjectStrip").then(({ ProjectStrip }) => ({ default: ProjectStrip })));

/* Memoised so switching tabs, which renders every window of the project again, leaves each chat as it was. */
export const AgentChatPane = memo(function AgentChatPane({
    agent,
    profile,
    cwd,
    active,
    visible = active,
    onBusyChange,
}: {
    agent: Agent;
    profile?: ProviderProfile;
    cwd: string;
    active: boolean;
    visible?: boolean;
    onBusyChange: (busy: boolean) => void;
}) {
    const home = useStore((s) => s.home);
    const [state, dispatch] = useReducer(chatReducer, initialChatState);
    const [foldMemory] = useState(newFoldMemory);
    const [composerError, setComposerError] = useState<string | null>(null);
    const { agentRef, sessionIdRef, recovery, retry, replaying, changingPermissions, appliedPermissionMode, permissionMode, earlier, loadEarlier } =
        useAcpSession({
            active,
            visible,
            agent,
            profile,
            cwd,
            connection: state.connection,
            foldMemory,
            dispatch,
            onError: setComposerError,
        });

    /* A resumed session sends its history back over many frames. The pane keeps
       showing what it had until the history is all in and its text has been
       read, then shows it at once, each row drawn whole in the frame it mounts. */
    const reading = useReadAfterReplay(replaying, state.messages);
    const holding = replaying || reading;
    const displayStateRef = useRef(state);
    if (visible) displayStateRef.current = holding ? heldTranscript(displayStateRef.current, state) : state;
    const displayState = displayStateRef.current;
    const subagentCards = useMemo(() => subagentsById(displayState.messages), [displayState.messages]);
    const [replyingPermission, setReplyingPermission] = useState<string | null>(null);
    const [stoppingTasks, setStoppingTasks] = useState<string[]>([]);
    const [foldedLive, setFoldedLive] = useState<ReadonlySet<string>>(() => new Set());
    const liveFolds = useMemo(
        () => ({
            folded: foldedLive,
            toggle: (kind: string) =>
                setFoldedLive((folded) => {
                    const next = new Set(folded);
                    if (!next.delete(kind)) next.add(kind);
                    return next;
                }),
        }),
        [foldedLive],
    );
    const paneRef = useRef<HTMLDivElement>(null);
    const scrollRef = useRef<HTMLDivElement>(null);
    const scrollContentRef = useRef<HTMLDivElement>(null);
    const agentLockedRef = useRef(false);
    if (state.messages.length > 0) agentLockedRef.current = true;
    const [changingConfig, setChangingConfig] = useState(false);
    const configPending = useRef(false);

    useMessageArrival(scrollRef, displayState.messages);
    useEffect(() => void warmTranscript(state.messages), [state.messages]);

    const { atBottom, onScroll, jumpToBottom } = useStickToBottom({ scrollRef, contentRef: scrollContentRef, visible });

    useEffect(() => onBusyChange(state.running), [onBusyChange, state.running]);

    const steerable = state.capabilities.steering === true;

    const { queued, send, steer, drop } = usePromptQueue({
        agentRef,
        agentId: agent.id,
        cwd,
        connection: state.connection,
        running: state.running,
        replaying,
        queued: state.queued,
        commands: state.commands,
        steerable,
        dispatch,
        onError: setComposerError,
    });
    const started = state.messages.length > 0 || Boolean(agent.resumeId);
    const worktree = useChatWorktree({
        agent,
        cwd,
        visible,
        started,
        connection: state.connection,
        running: state.running,
        send,
        onError: setComposerError,
    });
    const sentHistory = useMemo(
        () =>
            sentPrompts(
                state.messages,
                queued.map((message) => message.text),
            ),
        [state.messages, queued],
    );

    const scrollByReader = useCallback((deltaY: number) => scrollRef.current?.scrollBy({ top: deltaY }), []);

    // Find opens on its shortcut while this chat is the pane in use; it counts up so asking again refocuses it.
    const [findRequest, setFindRequest] = useState(0);
    useEffect(() => {
        if (!active) return;
        const onKey = (event: KeyboardEvent) => {
            if (event.defaultPrevented || event.shiftKey || event.altKey || event.code !== "KeyF" || !hasPrimaryModifier(event)) return;
            event.preventDefault();
            setFindRequest((count) => count + 1);
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, [active]);

    const editable = state.capabilities.editing === true && state.connection === "ready" && !state.running;
    const mates = useMates(agent.id);
    const editMessage = useCallback(
        (message: ChatMessage, text: string) => {
            const messageId = message.promptId;
            if (!messageId) return;
            const paths = message.attachments ?? [];
            setComposerError(null);
            dispatch({ type: "rewind", messageId });
            dispatch({ type: "local_prompt", text, paths, messageId });
            acpApi.edit(agentRef.current.id, messageId, text, paths, mentionContext(text, mates)).catch((error: unknown) => {
                dispatch({ type: "error", message: errorText(error) });
            });
        },
        [agentRef, dispatch, mates],
    );

    const questions = useChatQuestions(agent.id, state.messages, setComposerError);
    const focusComposer = () => paneRef.current?.querySelector<HTMLElement>(".chat-composer .chat-draft")?.focus();
    const writeAnswer = (question: AgentQuestion) => {
        questions.write(question);
        window.requestAnimationFrame(focusComposer);
    };
    const sendMessage = (message: OutgoingMessage, steerNow: boolean): boolean => {
        const question = questions.composing;
        if (!question) return worktree.sendMessage(message, steerNow);
        const text = answerText(message.text, message.paths, message.context);
        if (!text) return false;
        if (!questions.writing) {
            questions.pick(question, { answer: { answer: text }, shown: text });
            return true;
        }
        void questions.answer(question, { answer: text }, text).then((answered) => {
            if (!answered) deliverToAgent(agent.id, message);
        });
        return true;
    };

    /* Messages waiting behind the turn come back to the composer rather than going out. */
    const stop = () => {
        putDraft(
            agent.id,
            queued
                .map((message) => message.text)
                .filter(Boolean)
                .join("\n\n"),
        );
        void acpApi.cancel(agent.id).catch((failure: unknown) => setComposerError(errorText(failure)));
    };

    const stopTask = async (taskId: string) => {
        setStoppingTasks((current) => [...current, taskId]);
        try {
            await acpApi.stopTask(agent.id, taskId);
        } catch (error) {
            setComposerError(errorText(error));
        } finally {
            setStoppingTasks((current) => current.filter((candidate) => candidate !== taskId));
        }
    };

    const replyPermission = async (requestId: string, optionId?: string) => {
        setReplyingPermission(requestId);
        try {
            await acpApi.permissionReply(agent.id, requestId, optionId);
            dispatch({ type: "permission_cleared", requestId });
        } catch (error) {
            const message = errorText(error);
            if (/no longer pending/.test(message)) dispatch({ type: "permission_cleared", requestId });
            else setComposerError(message);
        } finally {
            setReplyingPermission(null);
        }
    };

    const changeConfig = async (config: SessionConfig, value: string) => {
        if (configPending.current || state.running || changingPermissions) return;
        configPending.current = true;
        setChangingConfig(true);
        setComposerError(null);
        const sessionId = sessionIdRef.current;
        try {
            const response = await acpApi.setConfig(agent.id, config.id, value);
            if (sessionIdRef.current !== sessionId) return;
            dispatch({ type: "config", options: response.configOptions });
            const options = sessionConfigs({ configOptions: response.configOptions });
            const model = options.find((option) => option.id === "model")?.currentValue ?? agent.model;
            const effort = effortConfig(options)?.currentValue ?? agent.effort;
            cmd.setAgentModelPreferences(agent.id, model, knownEffort(effort));
        } catch (error) {
            if (sessionIdRef.current === sessionId) setComposerError(errorText(error));
        } finally {
            configPending.current = false;
            setChangingConfig(false);
        }
    };
    const activeTool = useMemo(() => activeToolLabel(displayState.messages), [displayState.messages]);
    const subagents = useMemo(() => runningSubagents(displayState.messages), [displayState.messages]);
    const connecting = connectingLabel(displayState.connection);
    const activity = recovery === null ? activityText(displayState, activeTool) : null;
    const disconnected = displayState.connection === "error" || displayState.connection === "stopped";
    const resuming = recovery?.phase === "resuming";
    const failure = recovery?.phase === "failed" ? recovery : null;
    const recoveryLabel = resuming ? "Resuming…" : failure ? "Couldn't resume this chat" : null;
    const welcoming = displayState.messages.length === 0 && (displayState.connection === "ready" || (!agent.resumeId && !disconnected));
    const chatAgent = useMemo(() => ({ id: agent.id, type: agent.type }), [agent.id, agent.type]);
    const composerPlaceholder =
        questions.composing && !questions.writing
            ? `Or answer “${questionTitle(questions.composing, questions.open.indexOf(questions.composing), questions.open.length)}” in your own words`
            : questions.writing && questions.composing
              ? `Answer ${questions.composing.name}`
              : placeholderFor(state, {
                    resuming,
                    disconnected,
                    commands: state.connection !== "ready" || state.commands.length > 0,
                });
    const sessionActions = <ResumeActions agent={agent} profile={profile} cwd={cwd} retry={failure ? "Retry" : "Reconnect"} onRetry={retry} />;
    const failureDetail = failure?.detail && <span className="chat-recovery-detail">{failure.detail}</span>;

    return (
        <PathRootsProvider cwd={cwd} home={home} agentId={chatAgent.id} host={agent.host}>
            <ChatAgentContext.Provider value={chatAgent}>
                <ChatSubagentsContext.Provider value={subagentCards}>
                    <ReaderScrollContext.Provider value={scrollByReader}>
                        <div className="agent-chat-pane" ref={paneRef}>
                            {findRequest > 0 && (
                                <Suspense fallback={null}>
                                    <ChatFind
                                        request={findRequest}
                                        visible={visible}
                                        messages={displayState.messages}
                                        scrollRef={scrollRef}
                                        onClose={() => {
                                            setFindRequest(0);
                                            focusComposer();
                                        }}
                                    />
                                </Suspense>
                            )}
                            <ChatTranscript
                                state={displayState}
                                agentType={agent.type}
                                foldMemory={foldMemory}
                                unfolded={findRequest > 0}
                                scrollRef={scrollRef}
                                contentRef={scrollContentRef}
                                onScroll={onScroll}
                                activity={activity}
                                permissions={{
                                    requests: displayState.permissions,
                                    busy: replyingPermission,
                                    onReply: (requestId, optionId) => void replyPermission(requestId, optionId),
                                }}
                                onEdit={editable ? editMessage : undefined}
                                before={
                                    <>
                                        {earlier !== "none" && <EarlierTurns state={earlier} onLoad={loadEarlier} scrollRef={scrollRef} />}
                                        {(worktree.step !== null || agent.worktree) && (
                                            <Suspense fallback={null}>
                                                <WorktreeNote step={worktree.step} worktree={agent.worktree} home={home} />
                                            </Suspense>
                                        )}
                                        {welcoming && <ChatWelcome cwd={cwd} agent={agent} />}
                                        {displayState.messages.length === 0 && !welcoming && (
                                            <div className={`chat-connection-state ${displayState.connection}`} role="status">
                                                {(connecting || resuming) && <span className="chat-activity-loader" aria-hidden="true" />}
                                                <span>
                                                    {recoveryLabel ??
                                                        connecting ??
                                                        (displayState.connection === "error"
                                                            ? "Structured session unavailable."
                                                            : "Agent session stopped.")}
                                                </span>
                                                {failureDetail}
                                                {disconnected && !resuming && sessionActions}
                                            </div>
                                        )}
                                    </>
                                }
                                after={
                                    <>
                                        {displayState.error && recovery === null && (
                                            <div className="chat-error" role="alert">
                                                <IconWarning size={14} />
                                                <span>
                                                    {displayState.failure?.kind === "network" ? lostConnectionText(agent.type) : displayState.error}
                                                </span>
                                                {displayState.failure?.kind === "network" ? (
                                                    <NetworkFailureActions
                                                        agent={agent}
                                                        failure={displayState.failure}
                                                        onDismiss={() => dispatch({ type: "dismiss_error" })}
                                                    />
                                                ) : (
                                                    displayState.failure && (
                                                        <ChatFailureActions
                                                            agent={agent}
                                                            profile={profile}
                                                            failure={displayState.failure}
                                                            signInMethods={state.capabilities.signInMethods as SignInMethod[] | undefined}
                                                        />
                                                    )
                                                )}
                                            </div>
                                        )}
                                        {(questions.assumed.length > 0 || questions.answered.length > 0) && (
                                            <QuestionRows questions={questions} onWrite={writeAnswer} />
                                        )}
                                        {displayState.messages.length > 0 && (resuming || disconnected) && (
                                            <div className="chat-reconnect" role="status">
                                                {resuming ? <span className="chat-activity-loader" aria-hidden="true" /> : <IconPlug size={13} />}
                                                <span>{recoveryLabel ?? "This session dropped."}</span>
                                                {failureDetail}
                                                {!resuming && sessionActions}
                                            </div>
                                        )}
                                    </>
                                }
                            />

                            <div className="chat-composer-wrap">
                                {!atBottom && displayState.messages.length > 0 && (
                                    <button type="button" className="chat-jump-bottom" aria-label="Jump to latest message" onClick={jumpToBottom}>
                                        <IconArrowDown size={14} />
                                    </button>
                                )}
                                <ComposerBuddy agent={agent} />
                                {(questions.open.length > 0 || subagents.length > 0 || displayState.tasks.length > 0 || queued.length > 0) && (
                                    <LiveStack
                                        questions={<QuestionDock questions={questions} active={active} paneRef={paneRef} />}
                                        subagents={subagents}
                                        tasks={displayState.tasks}
                                        folds={liveFolds}
                                        stopping={stoppingTasks}
                                        onStop={(taskId) => void stopTask(taskId)}
                                        queue={{
                                            messages: queued,
                                            steerable: steerable && state.running,
                                            onSteer: (messages) => void steer(messages),
                                            onDrop: drop,
                                        }}
                                    />
                                )}
                                {!started && !agent.worktree && worktree.step === null && (
                                    <Suspense fallback={null}>
                                        <ProjectStrip agentId={agent.id} cwd={cwd} worktree={worktree} />
                                    </Suspense>
                                )}
                                <ChatComposer
                                    agent={agent}
                                    profile={profile}
                                    paneRef={paneRef}
                                    visible={visible}
                                    connection={state.connection}
                                    running={state.running}
                                    steerable={steerable}
                                    commands={state.commands}
                                    setup={state.setup}
                                    awaitingPermission={state.permissions.length > 0}
                                    agentLocked={agentLockedRef.current}
                                    changingConfig={changingConfig}
                                    changingPermissions={changingPermissions}
                                    permissionApplied={state.connection !== "ready" || permissionMode === appliedPermissionMode}
                                    placeholder={composerPlaceholder}
                                    error={composerError}
                                    onError={setComposerError}
                                    onSend={sendMessage}
                                    onSteerQueued={() => {
                                        if (queued.length > 0) void steer(queued);
                                    }}
                                    onStop={stop}
                                    queuedCount={queued.length}
                                    usage={state.usage}
                                    onConfig={changeConfig}
                                    history={sentHistory}
                                    worktree={worktree}
                                    answering={
                                        questions.writing && questions.composing
                                            ? {
                                                  agentId: questions.composing.agentId,
                                                  name: questions.composing.name,
                                                  cli: questions.composing.cli,
                                                  question: questions.composing.text,
                                                  onRemove: questions.stopComposing,
                                              }
                                            : null
                                    }
                                    onQuestionKey={questions.open.length > 0 ? questions.key : undefined}
                                />
                            </div>
                            <div className="chat-drop-target" aria-hidden="true">
                                <IconFile size={22} />
                                <span>Drop files or folders into this session</span>
                            </div>
                        </div>
                    </ReaderScrollContext.Provider>
                </ChatSubagentsContext.Provider>
            </ChatAgentContext.Provider>
        </PathRootsProvider>
    );
});
