import { useCallback, useEffect, useRef, useState, type Dispatch } from "react";
import { acpApi, type AcpEvent, type AcpStartResponse } from "../api/acp";
import type { Agent, AgentPermissionMode, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import type { FoldMemory } from "./longText";
import { eventMessage, permissionRequest, promptAction, recordOf, statusFromEvent } from "./acpEvents";
import { permissionModeOf } from "./chatStatus";
import { afterSessionEnd, sessionEndOf, type Recovery } from "./sessionRecovery";
import { claimChat } from "./chatClaims";
import type { ChatAction, ChatState } from "./types";

const UPDATE_FLUSH_FALLBACK_MS = 250;

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
    const lastResumeAtRef = useRef<number | null>(null);
    const queuedUpdatesRef = useRef<[string, Record<string, unknown>][]>([]);
    const updateFrameRef = useRef<number | null>(null);
    const updateTimerRef = useRef<number | null>(null);
    const agentRef = useRef(agent);
    agentRef.current = agent;
    const sessionIdRef = useRef<string | null>(null);
    const lifecycleRef = useRef<Promise<unknown>>(Promise.resolve());
    /* Set when the core came back still running this chat: the session is
       taken up again rather than stopped and started. */
    const reattachingRef = useRef(false);
    const [changingPermissions, setChangingPermissions] = useState(false);
    const [appliedPermissionMode, setAppliedPermissionMode] = useState<string | null>(null);
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
        lastResumeAtRef.current = Date.now();
        updateRecovery(agentRef.current.resumeId ? { phase: "resuming" } : null);
        setRestartKey((value) => value + 1);
    }, [updateRecovery]);

    useEffect(() => {
        if (!active) return;
        const controller = new AbortController();
        let mounted = true;
        const reattaching = reattachingRef.current;
        reattachingRef.current = false;
        const hold = Boolean(agentRef.current.resumeId) || reattaching;
        dispatch({ type: "reset", hold });
        if (!hold) {
            foldMemory.streamed.clear();
            foldMemory.expanded.clear();
        }
        setAppliedPermissionMode(null);
        setChangingPermissions(false);
        sessionIdRef.current = null;

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

        /* An agent that dies under the chat — a crash, a rate limit, a laptop
           waking up — comes back on its saved session. */
        const noteEnd = (event: AcpEvent) => {
            const end = sessionEndOf(event);
            if (!end) return;
            const now = Date.now();
            const next = afterSessionEnd({
                end,
                resumeId: agentRef.current.resumeId,
                resuming: recoveryRef.current?.phase === "resuming",
                lastResumeAt: lastResumeAtRef.current,
                now,
            });
            if (next === "resume") {
                lastResumeAtRef.current = now;
                updateRecovery({ phase: "resuming" });
                setRestartKey((value) => value + 1);
            } else if (next === "give-up") {
                updateRecovery({ phase: "failed", detail: typeof event.payload.message === "string" ? event.payload.message : null });
            }
        };

        const handleEvent = (event: AcpEvent) => {
            if (!mounted || event.agentId !== agent.id) return;
            if (event.kind !== "session_update") flushUpdates();
            if (event.kind === "status") {
                dispatch({ type: "status", state: statusFromEvent(event) });
                noteEnd(event);
            } else if (event.kind === "ready") {
                dispatch({
                    type: "ready",
                    capabilities: recordOf(event.payload.capabilities) ?? {},
                    setup: recordOf(event.payload.setup) ?? {},
                });
            } else if (event.kind === "session_update") {
                const batch = Array.isArray(event.payload.updates) ? event.payload.updates : [];
                for (const entry of batch) {
                    const row = recordOf(entry);
                    if (!row) continue;
                    const update = recordOf(row.update);
                    const sessionId = typeof row.sessionId === "string" ? row.sessionId : null;
                    if (update && sessionId) queueUpdate(sessionId, update);
                }
            } else if (event.kind === "prompt") {
                const prompted = promptAction(event.payload);
                if (prompted) dispatch(prompted);
            } else if (event.kind === "turn_started") {
                if (sessionIdRef.current && agentRef.current.resumeId !== sessionIdRef.current) {
                    cmd.attachAgentSession(agent.id, sessionIdRef.current);
                }
                dispatch({ type: "turn_started" });
            } else if (event.kind === "turn_completed") {
                dispatch({
                    type: "turn_completed",
                    stopReason: typeof event.payload.stopReason === "string" ? event.payload.stopReason : undefined,
                });
            } else if (event.kind === "permission_request") {
                const request = permissionRequest(event.payload);
                if (request) dispatch({ type: "permission_requested", request });
            } else if (event.kind === "error") dispatch({ type: "error", message: eventMessage(event) });
            else if (event.kind === "reattach") {
                reattachingRef.current = true;
                setRestartKey((value) => value + 1);
            }
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
                if (attached.status === "live") {
                    response = attached.start;
                    appliedMode = attached.permissionMode;
                    if (attached.turned && current.resumeId !== response.sessionId) cmd.attachAgentSession(current.id, response.sessionId);
                } else {
                    if (attached.status === "restart") await acpApi.stop(current.id);
                    if (!mounted) return;
                    appliedMode = permissionModeOf(current);
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
                    });
                }
                if (!mounted) return;
                sessionIdRef.current = response.sessionId;
                flushUpdates();
                dispatch({ type: "ready", capabilities: response.capabilities, setup: response.setup });
                setAppliedPermissionMode(appliedMode);
            })
            .catch((error: unknown) => {
                if (!controller.signal.aborted && mounted) {
                    const message = error instanceof Error ? error.message : String(error);
                    dispatch({ type: "error", message });
                    if (recoveryRef.current?.phase === "resuming") updateRecovery({ phase: "failed", detail: message });
                }
            });

        lifecycleRef.current = lifecycle;
        return () => {
            mounted = false;
            sessionIdRef.current = null;
            if (updateFrameRef.current !== null) window.cancelAnimationFrame(updateFrameRef.current);
            updateFrameRef.current = null;
            if (updateTimerRef.current !== null) window.clearTimeout(updateTimerRef.current);
            updateTimerRef.current = null;
            queuedUpdatesRef.current = [];
            controller.abort();
            lifecycleRef.current = reattachingRef.current ? lifecycle : lifecycle.finally(() => acpApi.stop(agent.id).catch(() => {}));
        };
    }, [
        active,
        agent.id,
        agent.type,
        agent.profileId,
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
                onError(error instanceof Error ? error.message : String(error));
            })
            .finally(() => {
                if (sessionIdRef.current === sessionId) setChangingPermissions(false);
            });
    }, [agent.id, connection, permissionMode, appliedPermissionMode, changingPermissions, onError]);

    return { agentRef, sessionIdRef, recovery, retry, changingPermissions, appliedPermissionMode, permissionMode };
}
