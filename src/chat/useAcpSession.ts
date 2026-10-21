import { useCallback, useEffect, useRef, useState, type Dispatch } from "react";
import { errorText } from "../lib/format";
import { acpApi, type AcpEvent } from "../api/acp";
import type { Agent, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { noteSignedOut } from "../agents/statusRefresh";
import { swallow } from "../state/toast";
import type { FoldMemory } from "./longText";
import { chatActions } from "./acpEvents";
import { earlierMessages, PAGE_TURNS, reachesSubagent } from "./earlier";
import { effectiveMode as permissionModeOf } from "../agents/agentLaunch";
import { afterSessionEnd, sessionEndOf, type Recovery } from "./sessionRecovery";
import { workspaceSent } from "../state/workspaceSent";
import type { ChatAction, ChatState } from "./types";

const UPDATE_FLUSH_FALLBACK_MS = 250;
/* A chat nobody is looking at takes in what streamed this often instead of every frame, and all of it the moment it shows. */
const HIDDEN_FLUSH_MS = 1000;

/* Where paging back through a chat the core sent only the end of goes next,
   and what subagents said since, which an earlier page's subagents play on into. */
type History = { cursor: { feed: string; before: number } | null; pages: number; held: ChatAction[] };

export type EarlierTurns = "none" | "idle" | "loading" | "failed";

export function useAcpSession({
    active,
    visible = active,
    agent,
    profile,
    cwd,
    connection,
    foldMemory,
    dispatch,
    onError,
}: {
    active: boolean;
    visible?: boolean;
    agent: Agent;
    profile?: ProviderProfile;
    cwd: string;
    connection: ChatState["connection"];
    foldMemory: FoldMemory;
    dispatch: Dispatch<ChatAction>;
    onError: (message: string | null) => void;
}) {
    const [restartKey, setRestartKey] = useState(0);
    const [recovery, setRecovery] = useState<Recovery | null>(null);
    const recoveryRef = useRef<Recovery | null>(null);
    const queuedUpdatesRef = useRef<[string, Record<string, unknown>][]>([]);
    const updateFrameRef = useRef<number | null>(null);
    const updateTimerRef = useRef<number | null>(null);
    const flushUpdatesRef = useRef<(() => void) | null>(null);
    const visibleRef = useRef(visible);
    visibleRef.current = visible;
    const agentRef = useRef(agent);
    agentRef.current = agent;
    const profileRef = useRef(profile);
    profileRef.current = profile;
    const sessionIdRef = useRef<string | null>(null);
    const lifecycleRef = useRef<Promise<unknown>>(Promise.resolve());
    /* What the chat was last started with; the person changing it, as in an
       empty chat's agent picker, starts it again on the new launch. Its
       account is not part of it: the core moves a chat between accounts. */
    const launchRef = useRef<string | null>(null);
    /* Set when the core came back still running this chat: the session is
       taken up again rather than stopped and started. */
    const reattachingRef = useRef(false);
    const [changingPermissions, setChangingPermissions] = useState(false);
    const [appliedPermissionMode, setAppliedPermissionMode] = useState<string | null>(null);
    /* True while a resumed or reattached session is sending its history back,
       which ends when starting or attaching it answers. */
    const [replaying, setReplaying] = useState(false);
    const historyRef = useRef<History | null>(null);
    const [earlier, setEarlier] = useState<EarlierTurns>("none");
    const permissionMode = permissionModeOf(agent);

    const updateRecovery = useCallback((next: Recovery | null) => {
        recoveryRef.current = next;
        setRecovery(next);
    }, []);

    useEffect(() => {
        if (connection === "ready") updateRecovery(null);
    }, [connection, updateRecovery]);

    const retry = useCallback(() => {
        updateRecovery(agentRef.current.resumeId ? { phase: "resuming" } : null);
        setRestartKey((value) => value + 1);
    }, [updateRecovery]);

    useEffect(() => {
        if (!active) return;
        const controller = new AbortController();
        let mounted = true;
        const reattaching = reattachingRef.current;
        reattachingRef.current = false;
        const launch = JSON.stringify([agent.type, agent.modelProvider, cwd]);
        const relaunch = launchRef.current !== null && launchRef.current !== launch && !reattaching;
        launchRef.current = launch;
        const hold = Boolean(agentRef.current.resumeId) || reattaching;
        dispatch({ type: "reset", hold });
        setReplaying(hold);
        if (!hold) {
            foldMemory.streamed.clear();
            foldMemory.expanded.clear();
        }
        setAppliedPermissionMode(null);
        setChangingPermissions(false);
        sessionIdRef.current = null;
        historyRef.current = { cursor: null, pages: 0, held: [] };
        setEarlier("none");

        const flushUpdates = () => {
            if (updateFrameRef.current !== null) {
                window.cancelAnimationFrame(updateFrameRef.current);
                updateFrameRef.current = null;
            }
            if (updateTimerRef.current !== null) {
                window.clearTimeout(updateTimerRef.current);
                updateTimerRef.current = null;
            }
            const updates = queuedUpdatesRef.current.splice(0);
            for (const [sessionId, update] of updates)
                dispatch({ type: "session_update", sessionId, update, main: sessionId === sessionIdRef.current });
        };
        flushUpdatesRef.current = flushUpdates;

        /* WebKit stops animation frames for a window that is hidden or behind
           another app, and only the first of those shows in document.hidden.
           A timer keeps the queue draining either way. */
        const queueUpdate = (sessionId: string, update: Record<string, unknown>) => {
            queuedUpdatesRef.current.push([sessionId, update]);
            const shown = visibleRef.current;
            if (updateTimerRef.current === null)
                updateTimerRef.current = window.setTimeout(flushUpdates, shown ? UPDATE_FLUSH_FALLBACK_MS : HIDDEN_FLUSH_MS);
            if (shown && !document.hidden && updateFrameRef.current === null) updateFrameRef.current = window.requestAnimationFrame(flushUpdates);
        };

        const noteEnd = (event: AcpEvent) => {
            const end = sessionEndOf(event);
            if (end && afterSessionEnd(end) === "give-up")
                updateRecovery({ phase: "failed", detail: typeof event.payload.message === "string" ? event.payload.message : null });
        };

        const handleEvent = (event: AcpEvent) => {
            if (!mounted || event.agentId !== agent.id) return;
            if (event.kind !== "session_update") flushUpdates();
            if (event.kind === "reattach") {
                reattachingRef.current = true;
                setRestartKey((value) => value + 1);
                return;
            }
            /* The core starts the agent again under a running chat after a
               sign-in or on another account, sometimes on a new session. */
            if (event.kind === "ready" && typeof event.payload.sessionId === "string" && sessionIdRef.current)
                sessionIdRef.current = event.payload.sessionId;
            const actions = chatActions(event);
            const history = historyRef.current;
            if (history) for (const action of actions) if (reachesSubagent(action, sessionIdRef.current)) history.held.push(action);
            for (const action of actions) {
                if (action.type === "session_update") {
                    const { update } = action;
                    queueUpdate(action.sessionId, update);
                    /* The saved history is all in while the agent is still starting,
                       so the transcript shows now rather than when the agent answers. */
                    if (update.sessionUpdate === "history_loaded" && !reattaching) {
                        flushUpdates();
                        setReplaying(false);
                    }
                    continue;
                }
                dispatch(action);
                if (action.type === "error" && action.failure?.kind === "signIn")
                    void noteSignedOut(agentRef.current.type, profileRef.current?.configPath).catch(swallow("mark agent signed out"));
            }
            if (event.kind === "status") noteEnd(event);
        };

        const lifecycle = lifecycleRef.current
            .catch(() => {})
            .then(async () => {
                if (!mounted) return;
                if (relaunch) await acpApi.stop(agent.id).catch(swallow("stop the chat agent"));
                await acpApi.subscribe(handleEvent, controller.signal);
                if (!mounted) return;
                const current = agentRef.current;
                /* A chat the core kept running through a reload or a quit is
                   taken up where it is: its replay rebuilds the transcript the
                   same way a resumed session's history does. */
                const attach = () => acpApi.attach(current.id);
                let attached = await attach();
                if (!mounted) return;
                if (attached.status !== "live") {
                    await workspaceSent();
                    await acpApi.startChat(current.id);
                    if (!mounted) return;
                    attached = await attach();
                    if (!mounted) return;
                    if (attached.status !== "live") throw new Error("The chat stopped as soon as it started");
                }
                const history = historyRef.current;
                if (attached.olderBefore !== null && history) {
                    history.cursor = { feed: attached.feed, before: attached.olderBefore };
                    history.held = history.held.filter((action) => reachesSubagent(action, attached.start.sessionId));
                    setEarlier("idle");
                } else historyRef.current = null;
                const response = attached.start;
                const appliedMode = attached.permissionMode;
                if (!mounted) return;
                sessionIdRef.current = response.sessionId;
                flushUpdates();
                dispatch({
                    type: "ready",
                    capabilities: response.capabilities,
                    setup: response.setup,
                    running: attached.running,
                });
                setReplaying(false);
                setAppliedPermissionMode(appliedMode);
            })
            .catch((error: unknown) => {
                if (!controller.signal.aborted && mounted) {
                    flushUpdates();
                    setReplaying(false);
                    const message = errorText(error);
                    dispatch({ type: "error", message });
                    if (recoveryRef.current?.phase === "resuming") updateRecovery({ phase: "failed", detail: message });
                }
            });

        lifecycleRef.current = lifecycle;
        return () => {
            mounted = false;
            historyRef.current = null;
            setReplaying(false);
            sessionIdRef.current = null;
            if (updateFrameRef.current !== null) window.cancelAnimationFrame(updateFrameRef.current);
            updateFrameRef.current = null;
            if (updateTimerRef.current !== null) window.clearTimeout(updateTimerRef.current);
            updateTimerRef.current = null;
            queuedUpdatesRef.current = [];
            flushUpdatesRef.current = null;
            controller.abort();
            lifecycleRef.current = lifecycle;
        };
    }, [active, agent.id, agent.type, agent.modelProvider, cwd, restartKey, foldMemory, dispatch, updateRecovery]);

    useEffect(() => {
        if (visible) flushUpdatesRef.current?.();
    }, [visible]);

    useEffect(() => {
        const sessionId = sessionIdRef.current;
        if (
            sessionId === null ||
            connection !== "ready" ||
            changingPermissions ||
            appliedPermissionMode === null ||
            permissionMode === appliedPermissionMode
        )
            return;
        setChangingPermissions(true);
        void acpApi
            .setPermissionMode(agent.id, permissionMode)
            .then(() => {
                if (sessionIdRef.current === sessionId) setAppliedPermissionMode(permissionMode);
            })
            .catch((error: unknown) => {
                if (sessionIdRef.current !== sessionId) return;
                if (permissionModeOf(agentRef.current) === permissionMode)
                    cmd.setAgentPermissionMode(agent.id, appliedPermissionMode as NonNullable<Agent["permissionMode"]>);
                onError(errorText(error));
            })
            .finally(() => {
                if (sessionIdRef.current === sessionId) setChangingPermissions(false);
            });
    }, [agent.id, connection, permissionMode, appliedPermissionMode, changingPermissions, onError]);

    const loadEarlier = useCallback(() => {
        const history = historyRef.current;
        const cursor = history?.cursor;
        if (!history || !cursor || earlier === "loading") return;
        setEarlier("loading");
        acpApi.history(agent.id, cursor.feed, cursor.before, PAGE_TURNS).then(
            (page) => {
                if (historyRef.current !== history) return;
                history.pages += 1;
                const actions = page.events.flatMap(chatActions);
                dispatch({ type: "earlier", messages: earlierMessages({ number: history.pages, actions }, history.held) });
                if (page.olderBefore === null) {
                    historyRef.current = null;
                    setEarlier("none");
                    return;
                }
                history.cursor = { feed: cursor.feed, before: page.olderBefore };
                history.held.unshift(...actions.filter((action) => reachesSubagent(action, sessionIdRef.current)));
                setEarlier("idle");
            },
            () => {
                if (historyRef.current === history) setEarlier("failed");
            },
        );
    }, [agent.id, dispatch, earlier]);

    return {
        agentRef,
        sessionIdRef,
        recovery,
        retry,
        replaying,
        changingPermissions,
        appliedPermissionMode,
        permissionMode,
        earlier,
        loadEarlier,
    };
}
