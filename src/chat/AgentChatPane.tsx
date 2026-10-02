import { lazy, Suspense, useEffect, useMemo, useReducer, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { acpApi } from "../api/acp";
import { effortConfig, sessionConfigs, type SessionConfig } from "./sessionConfig";
import { rowMeta } from "./messageMeta";
import type { Agent, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { hasPrimaryModifier } from "../lib/platform";
import { IconArrowDown, IconFile, IconPlug, IconWarning } from "../ui/Icons";
import { chatReducer, initialChatState } from "./reducer";
import { PathRootsProvider } from "./FileRef";
import { ChatWelcome } from "./ChatWelcome";
import { FoldMemoryContext, newFoldMemory } from "./longText";
import { sentPrompts } from "./promptHistory";
import { activeToolLabel } from "./toolLabels";
import { formatDetail, runningSubagents } from "./transcript";
import { activityText, backendState, composerPlaceholder as placeholderFor, connectingLabel, knownEffort } from "./chatStatus";
import { ChatAgentContext } from "./chatAgent";
import { ChatMessageRow } from "./ChatMessageRow";
import { ChatActivity } from "./ChatActivity";
import { PermissionRequest } from "./PermissionRequest";
import { BackgroundTasks, QueuedMessages, RunningSubagents } from "./LiveStack";
import { ChatComposer } from "./ChatComposer";
import { useMessageArrival } from "./useMessageArrival";
import { useAcpSession } from "./useAcpSession";
import { useSavedUsage } from "./useSavedUsage";
import { usePromptQueue } from "./usePromptQueue";
import { useChatWorktree } from "./useChatWorktree";
import { BOTTOM_SLACK, useStickToBottom } from "./useStickToBottom";

const ChatFind = lazy(() => import("./ChatFind"));
const WorktreeNote = lazy(() => import("./ChatWorktree").then(({ WorktreeNote }) => ({ default: WorktreeNote })));
const ProjectStrip = lazy(() => import("./ProjectStrip").then(({ ProjectStrip }) => ({ default: ProjectStrip })));

export function AgentChatPane({
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
    const displayStateRef = useRef(state);
    if (visible) displayStateRef.current = state;
    const displayState = displayStateRef.current;
    const [composerError, setComposerError] = useState<string | null>(null);
    const [replyingPermission, setReplyingPermission] = useState<string | null>(null);
    const [stoppingTasks, setStoppingTasks] = useState<string[]>([]);
    const paneRef = useRef<HTMLDivElement>(null);
    const scrollRef = useRef<HTMLDivElement>(null);
    const scrollContentRef = useRef<HTMLDivElement>(null);
    const agentLockedRef = useRef(false);
    if (state.messages.length > 0) agentLockedRef.current = true;
    const [changingConfig, setChangingConfig] = useState(false);
    const configPending = useRef(false);

    useMessageArrival(scrollRef, displayState.messages);

    /* A restored transcript opens on estimated row heights, and every row that
       measures taller or shorter than the estimate moves the bottom. Anchoring
       to the end makes the list hold the bottom still while that settles. */
    const virtualizer = useVirtualizer({
        count: displayState.messages.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 76,
        overscan: 8,
        anchorTo: "end",
        scrollEndThreshold: BOTTOM_SLACK,
        getItemKey: (index) => displayState.messages[index]?.id ?? index,
        /* Rows are placed from the resize observer itself, in the frame a row
           changes size, rather than on the render after. A row that grows or
           folds by animation then pushes the rest along with it instead of
           overlapping them for a frame and catching up. */
        directDomUpdates: true,
    });

    useEffect(() => {
        if (!active) return;
        cmd.noteAcpAgentState(
            agent.id,
            backendState({ connection: state.connection, awaitingPermission: state.permissions.length > 0, running: state.running }),
        );
    }, [active, agent.id, state.connection, state.running, state.permissions.length]);

    /* A turn ends long before the work it started does. Shells, monitors and
       subagents keep going after the answer, and they die with the agent, so
       what is still running is what says the agent is still in use. */
    const liveTasks = useMemo(() => state.tasks.filter((task) => task.state === "running").length, [state.tasks]);
    const liveSubagents = useMemo(() => runningSubagents(state.messages).length, [state.messages]);
    useEffect(() => cmd.noteAgentBackgroundWork(agent.id, liveTasks, liveSubagents), [agent.id, liveTasks, liveSubagents]);
    useEffect(() => () => cmd.noteAgentBackgroundWork(agent.id, 0, 0), [agent.id]);

    useEffect(() => onBusyChange(state.running), [onBusyChange, state.running]);

    const { agentRef, sessionIdRef, recovery, retry, changingPermissions, appliedPermissionMode, permissionMode } = useAcpSession({
        active,
        agent,
        profile,
        cwd,
        connection: state.connection,
        foldMemory,
        dispatch,
        onError: setComposerError,
    });

    useSavedUsage({
        agentRef,
        agentId: agent.id,
        agentResumeId: agent.resumeId,
        cwd,
        configPath: profile?.configPath,
        connection: state.connection,
        reported: state.usage !== null,
        setup: state.setup,
        dispatch,
    });

    useEffect(() => {
        if (state.title && state.title !== agent.title) cmd.setAgentTitle(agent.id, state.title);
    }, [agent.id, agent.title, state.title]);

    const steerable = state.capabilities.steering === true;

    const { queued, send, steer, drop } = usePromptQueue({
        agentRef,
        agentId: agent.id,
        cwd,
        connection: state.connection,
        running: state.running,
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

    const { atBottom, noteGesture, onScroll, jumpToBottom, leaveBottom } = useStickToBottom({
        scrollRef,
        contentRef: scrollContentRef,
        visible,
        messageCount: displayState.messages.length,
        revision: displayState.revision,
    });

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

    const stop = () => {
        void acpApi.cancel(agent.id).catch((failure: unknown) => setComposerError(failure instanceof Error ? failure.message : String(failure)));
    };

    const stopTask = async (taskId: string) => {
        setStoppingTasks((current) => [...current, taskId]);
        try {
            await acpApi.stopTask(agent.id, taskId);
        } catch (error) {
            setComposerError(error instanceof Error ? error.message : String(error));
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
            setComposerError(error instanceof Error ? error.message : String(error));
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
            const effort = effortConfig(options, agent.type)?.currentValue ?? agent.effort;
            cmd.setAgentModelPreferences(agent.id, model, knownEffort(effort));
        } catch (error) {
            if (sessionIdRef.current === sessionId) setComposerError(error instanceof Error ? error.message : String(error));
        } finally {
            configPending.current = false;
            setChangingConfig(false);
        }
    };
    const activeTool = useMemo(() => activeToolLabel(displayState.messages), [displayState.messages]);
    const subagents = useMemo(() => runningSubagents(displayState.messages), [displayState.messages]);
    const plan = useMemo(() => (displayState.plan === null ? null : formatDetail(displayState.plan)), [displayState.plan]);
    const connecting = connectingLabel(displayState.connection);
    const activity = recovery === null ? activityText(displayState, activeTool) : null;
    const disconnected = displayState.connection === "error" || displayState.connection === "stopped";
    const resuming = recovery?.phase === "resuming";
    const failure = recovery?.phase === "failed" ? recovery : null;
    const welcoming = displayState.messages.length === 0 && displayState.connection === "ready";
    const startNewChat = () =>
        cmd.addAgent(agent.type, undefined, undefined, {
            permissionMode: agent.permissionMode,
            profileId: agent.profileId,
            detectedExecutablePath: profile?.executablePath || agent.executablePath,
            cwd,
        });
    const chatAgent = useMemo(() => ({ id: agent.id, type: agent.type }), [agent.id, agent.type]);
    const composerPlaceholder = placeholderFor(state, { resuming, disconnected });
    const sessionActions = (
        <div className="chat-connection-actions">
            <button type="button" onClick={retry}>
                {failure ? "Retry" : "Reconnect"}
            </button>
            {agent.resumeId && (
                <button type="button" onClick={startNewChat}>
                    Start new chat
                </button>
            )}
        </div>
    );
    const failureDetail = failure?.detail && <span className="chat-recovery-detail">{failure.detail}</span>;

    return (
        <PathRootsProvider cwd={cwd} home={home} agentId={chatAgent.id}>
            <ChatAgentContext.Provider value={chatAgent}>
                <div className="agent-chat-pane" ref={paneRef}>
                    {findRequest > 0 && (
                        <Suspense fallback={null}>
                            <ChatFind
                                request={findRequest}
                                visible={visible}
                                messages={displayState.messages}
                                scrollRef={scrollRef}
                                virtualizer={virtualizer}
                                onLeaveBottom={leaveBottom}
                                onClose={() => {
                                    setFindRequest(0);
                                    paneRef.current?.querySelector<HTMLTextAreaElement>(".chat-composer textarea")?.focus();
                                }}
                            />
                        </Suspense>
                    )}
                    <div
                        className="chat-scroll"
                        ref={scrollRef}
                        onWheel={noteGesture}
                        onTouchMove={noteGesture}
                        onMouseDown={noteGesture}
                        onKeyDown={noteGesture}
                        onScroll={onScroll}>
                        <div className="chat-scroll-content" ref={scrollContentRef}>
                            {(worktree.step !== null || agent.worktree) && (
                                <Suspense fallback={null}>
                                    <WorktreeNote step={worktree.step} worktree={agent.worktree} home={home} />
                                </Suspense>
                            )}
                            {welcoming && <ChatWelcome cwd={cwd} agentType={agent.type} />}
                            {displayState.messages.length === 0 && !welcoming && (
                                <div className={`chat-connection-state ${displayState.connection}`} role="status">
                                    {(connecting || resuming) && <span className="chat-activity-loader" aria-hidden="true" />}
                                    <span>
                                        {resuming
                                            ? "Resuming…"
                                            : failure
                                              ? "Couldn't resume this chat"
                                              : (connecting ??
                                                (displayState.connection === "error" ? "Structured session unavailable." : "Agent session stopped."))}
                                    </span>
                                    {failureDetail}
                                    {disconnected && !resuming && sessionActions}
                                </div>
                            )}
                            <FoldMemoryContext value={foldMemory}>
                                <div className="chat-virtual-space" ref={virtualizer.containerRef}>
                                    {virtualizer.getVirtualItems().map((item) => {
                                        const message = displayState.messages[item.index];
                                        const meta = rowMeta(displayState.messages, item.index);
                                        return (
                                            <div
                                                key={message.id}
                                                data-index={item.index}
                                                ref={virtualizer.measureElement}
                                                className="chat-virtual-row">
                                                <ChatMessageRow
                                                    message={message}
                                                    live={displayState.running && item.index === displayState.messages.length - 1}
                                                    copyable={meta.text}
                                                    rate={meta.rate}
                                                    at={meta.at}
                                                    took={meta.took}
                                                />
                                            </div>
                                        );
                                    })}
                                </div>
                            </FoldMemoryContext>
                            {activity && <ChatActivity key={displayState.running ? "turn" : "connect"} label={activity} agentType={agent.type} />}
                            {plan !== null && (
                                <details className="chat-plan">
                                    <summary>Plan</summary>
                                    <pre>{plan}</pre>
                                </details>
                            )}
                            {displayState.permissions.map((request) => (
                                <PermissionRequest
                                    key={request.requestId}
                                    request={request}
                                    busy={replyingPermission === request.requestId}
                                    onReply={(optionId) => void replyPermission(request.requestId, optionId)}
                                />
                            ))}
                            {displayState.error && recovery === null && (
                                <div className="chat-error" role="alert">
                                    <IconWarning size={14} />
                                    <span>{displayState.error}</span>
                                </div>
                            )}
                            {displayState.messages.length > 0 && (resuming || disconnected) && (
                                <div className="chat-reconnect" role="status">
                                    {resuming ? <span className="chat-activity-loader" aria-hidden="true" /> : <IconPlug size={13} />}
                                    <span>{resuming ? "Resuming…" : failure ? "Couldn't resume this chat" : "This session dropped."}</span>
                                    {failureDetail}
                                    {!resuming && sessionActions}
                                </div>
                            )}
                        </div>
                    </div>

                    <div className="chat-composer-wrap">
                        {!atBottom && displayState.messages.length > 0 && (
                            <button type="button" className="chat-jump-bottom" aria-label="Jump to latest message" onClick={jumpToBottom}>
                                <IconArrowDown size={14} />
                            </button>
                        )}
                        {(subagents.length > 0 || displayState.tasks.length > 0 || queued.length > 0) && (
                            <div className="chat-live-stack">
                                <RunningSubagents subagents={subagents} />
                                <BackgroundTasks tasks={displayState.tasks} stopping={stoppingTasks} onStop={(taskId) => void stopTask(taskId)} />
                                <QueuedMessages
                                    messages={queued}
                                    steerable={steerable && state.running}
                                    onSteer={(messages) => void steer(messages)}
                                    onDrop={drop}
                                />
                            </div>
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
                            onSend={worktree.sendMessage}
                            onSteerQueued={() => {
                                if (queued.length > 0) void steer(queued);
                            }}
                            onStop={stop}
                            queuedCount={queued.length}
                            usage={state.usage}
                            onConfig={changeConfig}
                            history={sentHistory}
                            worktree={worktree}
                        />
                    </div>
                    <div className="chat-drop-target" aria-hidden="true">
                        <IconFile size={22} />
                        <span>Drop files or folders into this session</span>
                    </div>
                </div>
            </ChatAgentContext.Provider>
        </PathRootsProvider>
    );
}
