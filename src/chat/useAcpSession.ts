import { useCallback, useEffect, useRef, useState, type Dispatch } from "react";
import { errorText } from "../lib/format";
import { acpApi, type AcpEvent, type AcpStartResponse } from "../api/acp";
import type { Agent, AgentPermissionMode, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { getState } from "../state/store";
import { acpAccount, takeMove } from "../agents/accounts";
import { noteSignedOut } from "../agents/statusRefresh";
import { swallow } from "../state/toast";
import type { FoldMemory } from "./longText";
import { chatActions } from "./acpEvents";
import { earlierMessages, PAGE_TURNS, reachesSubagent } from "./earlier";
import { effectiveMode as permissionModeOf } from "../agents/agentLaunch";
import { afterSessionEnd, sessionEndOf, type Recovery } from "./sessionRecovery";
import { claimChat } from "./chatClaims";
import type { ChatAction, ChatState } from "./types";

const UPDATE_FLUSH_FALLBACK_MS = 250;

/* Where paging back through a chat the core sent only the end of goes next,
   and what subagents said since, which an earlier page's subagents play on into. */
type History = { cursor: { feed: string; before: number } | null; pages: number; held: ChatAction[] };

export type EarlierTurns = "none" | "idle" | "loading" | "failed";

export function useAcpSession({
    active,
    agent,
    profile,
    cwd,
    connection,
    foldMemory,
    dispatch,
    onError,
}: {
    active: boolean;
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
    const agentRef = useRef(agent);
    agentRef.current = agent;
    const profileRef = useRef(profile);
    profileRef.current = profile;
    const sessionIdRef = useRef<string | null>(null);
    const lifecycleRef = useRef<Promise<unknown>>(Promise.resolve());
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
    const environmentKeys = JSON.stringify(profile?.environmentKeys ?? []);
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
        takeMove(agent.id, undefined);
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
            for (const [sessionId, update] of updates) dispatch({ type: "session_update", sessionId, update });
        };

        /* WebKit stops animation frames for a window that is hidden or behind
           another app, and only the first of those shows in document.hidden.
           A timer keeps the queue draining either way. */
        const queueUpdate = (sessionId: string, update: Record<string, unknown>) => {
            queuedUpdatesRef.current.push([sessionId, update]);
            if (updateTimerRef.current === null) updateTimerRef.current = window.setTimeout(flushUpdates, UPDATE_FLUSH_FALLBACK_MS);
            if (!document.hidden && updateFrameRef.current === null) updateFrameRef.current = window.requestAnimationFrame(flushUpdates);
        };

        const noteEnd = (event: AcpEvent) => {
            const end = sessionEndOf(event);
            if (end && afterSessionEnd(end) === "give-up")
                updateRecovery({ phase: "failed", detail: typeof event.payload.message === "string" ? event.payload.message : null });
        };

        /* The chat already runs on the new account, so the agent is taken up
           where it is rather than started again on it. */
        const noteAccount = (profileId: string) => {
            const known = getState().providerProfiles.some((item) => item.id === profileId && item.provider === agentRef.current.type);
            if (!known || agentRef.current.profileId === profileId) return;
            reattachingRef.current = true;
            cmd.setAgentAccount(agent.id, profileId);
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
            if (event.kind === "turn_started" && sessionIdRef.current && agentRef.current.resumeId !== sessionIdRef.current)
                cmd.attachAgentSession(agent.id, sessionIdRef.current);
            const actions = chatActions(event);
            const history = historyRef.current;
            if (history) for (const action of actions) if (reachesSubagent(action, sessionIdRef.current)) history.held.push(action);
            for (const action of actions) {
                if (action.type === "session_update") {
                    const { update } = action;
                    if (update.sessionUpdate === "account_switched" && typeof update.account === "string") noteAccount(update.account);
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
                    void noteSignedOut(agentRef.current.type, profile?.configPath).catch(swallow("mark agent signed out"));
            }
            if (event.kind === "status") noteEnd(event);
        };

        const lifecycle = lifecycleRef.current
            .catch(() => {})
            .then(async () => {
                if (!mounted) return;
                claimChat(agentRef.current.id);
                await acpApi.subscribe(handleEvent, controller.signal);
                if (!mounted) return;
                const current = agentRef.current;
                /* A chat the core kept running through a reload or a quit is
                   taken up where it is: its replay rebuilds the transcript the
                   same way a resumed session's history does. */
                const attached = await acpApi.attach({ agentId: current.id, provider: current.type, cwd, configPath: profile?.configPath });
                if (!mounted) return;
                let response: AcpStartResponse;
                let appliedMode: AgentPermissionMode;
                const history = historyRef.current;
                if (attached.status === "live" && attached.olderBefore !== null && history) {
                    history.cursor = { feed: attached.feed, before: attached.olderBefore };
                    history.held = history.held.filter((action) => reachesSubagent(action, attached.start.sessionId));
                    setEarlier("idle");
                } else historyRef.current = null;
                if (attached.status === "live") {
                    response = attached.start;
                    appliedMode = attached.permissionMode;
                    if (attached.turned && current.resumeId !== response.sessionId) cmd.attachAgentSession(current.id, response.sessionId);
                } else {
                    appliedMode = permissionModeOf(current);
                    const account = profileRef.current;
                    response = await acpApi.start({
                        agentId: current.id,
                        provider: current.type,
                        cwd,
                        resumeId: current.resumeId,
                        permissionMode: appliedMode,
                        configPath: profile?.configPath,
                        executablePath: profile?.executablePath || current.executablePath,
                        model: current.model,
                        effort: current.effort,
                        environmentKeys: JSON.parse(environmentKeys) as string[],
                        account: account ? acpAccount(account) : undefined,
                        modelProvider: current.modelProvider,
                    });
                }
                if (!mounted) return;
                sessionIdRef.current = response.sessionId;
                flushUpdates();
                dispatch({ type: "ready", capabilities: response.capabilities, setup: response.setup });
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
            controller.abort();
            if (takeMove(agent.id, agentRef.current.profileId)) reattachingRef.current = true;
            lifecycleRef.current = reattachingRef.current ? lifecycle : lifecycle.finally(() => acpApi.stop(agent.id).catch(() => {}));
        };
    }, [
        active,
        agent.id,
        agent.type,
        agent.profileId,
        agent.modelProvider,
        agent.executablePath,
        cwd,
        profile?.configPath,
        profile?.executablePath,
        environmentKeys,
        restartKey,
        foldMemory,
        dispatch,
        updateRecovery,
    ]);

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
