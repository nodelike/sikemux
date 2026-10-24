import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorText } from "../lib/format";
import { acpApi, type AcpEvent, type PromptContext } from "../api/acp";
import type { Agent, ProviderProfile } from "../state/types";
import * as cmd from "../state/commands";
import { noteSignedOut } from "../agents/statusRefresh";
import { swallow } from "../state/toast";
import { effectiveMode as permissionModeOf } from "../agents/agentLaunch";
import { workspaceSent } from "../state/workspaceSent";
import { applyChanges, emptyChat, fromView, withPage, type ChatRows } from "./rows";
import type { ChatChange, ChatMessage } from "./types";

/** How many turns one trip back through a long chat brings from the host. */
const PAGE_TURNS = 20;
/* A chat nobody is looking at takes in what streamed this often instead of at once, and all of it the moment it shows. */
const HIDDEN_FLUSH_MS = 1000;

export type Recovery = { phase: "resuming" } | { phase: "failed"; detail: string | null };

export type EarlierTurns = "none" | "idle" | "loading" | "failed";

/** A message this app sent that the core has not shown back yet. */
interface Pending {
    messageId: string;
    text: string;
    paths: string[];
    context: PromptContext[];
    at: number;
}

function pendingRow({ messageId, text, paths, context, at }: Pending): ChatMessage {
    const id = `local-${messageId}`;
    return {
        id,
        role: "user",
        promptId: messageId,
        sentAt: at,
        parts: text.trim() ? [{ kind: "text", id: `${id}.0`, text }] : [],
        ...(paths.length ? { attachments: paths } : {}),
        ...(context.length ? { context: context.map(({ uri, title }) => ({ uri, title })) } : {}),
    };
}

/**
 * One chat as the core folds it, taken up while the pane is active: the view
 * it attaches to, the changes that follow, the pages before it, and what this
 * app sent that the core has not shown back yet.
 */
export function useAcpSession({
    active,
    visible = active,
    agent,
    profile,
    cwd,
    onError,
}: {
    active: boolean;
    visible?: boolean;
    agent: Agent;
    profile?: ProviderProfile;
    cwd: string;
    onError: (message: string | null) => void;
}) {
    const [restartKey, setRestartKey] = useState(0);
    const [recovery, setRecovery] = useState<Recovery | null>(null);
    const [held, setHeld] = useState<ChatRows>(emptyChat);
    const [pending, setPending] = useState<Pending[]>([]);
    const [dismissed, setDismissed] = useState<string | null>(null);
    const [earlier, setEarlier] = useState<EarlierTurns>("none");
    const [changingPermissions, setChangingPermissions] = useState(false);
    /* The mode this app last set, until the core says what it is now. */
    const [setMode, setSetMode] = useState<string | null>(null);
    const feedRef = useRef<string | null>(null);
    const flushRef = useRef<(() => void) | null>(null);
    const visibleRef = useRef(visible);
    visibleRef.current = visible;
    const agentRef = useRef(agent);
    agentRef.current = agent;
    const profileRef = useRef(profile);
    profileRef.current = profile;
    const lifecycleRef = useRef<Promise<unknown>>(Promise.resolve());
    /* What the chat was last started with; the person changing it, as in an
       empty chat's agent picker, starts it again on the new launch. */
    const launchRef = useRef<string | null>(null);
    const reattachingRef = useRef(false);
    /* A chat whose agent is not running stays that way when shown: only the person's message,
       or their asking it to reconnect, starts it. */
    const dormantRef = useRef(false);
    const startWantedRef = useRef(false);
    const permissionMode = permissionModeOf(agent);

    useEffect(() => {
        if (held.connection === "ready") setRecovery(null);
    }, [held.connection]);

    useEffect(() => {
        const ended = held.ended;
        if (ended?.reason === "exited") setRecovery({ phase: "failed", detail: ended.message ?? null });
    }, [held.ended]);

    useEffect(() => {
        if (held.failure?.kind === "signIn")
            void noteSignedOut(agentRef.current.type, profileRef.current?.configPath).catch(swallow("mark agent signed out"));
    }, [held.failure]);

    const retry = useCallback(() => {
        startWantedRef.current = true;
        setRecovery(agentRef.current.resumeId ? { phase: "resuming" } : null);
        setRestartKey((value) => value + 1);
    }, []);

    useEffect(() => {
        if (!active) return;
        const controller = new AbortController();
        let mounted = true;
        const reattaching = reattachingRef.current;
        reattachingRef.current = false;
        const launch = JSON.stringify([agent.type, agent.modelProvider, cwd]);
        const relaunch = launchRef.current !== null && launchRef.current !== launch && !reattaching;
        launchRef.current = launch;
        if (!reattaching) setHeld(emptyChat);
        setEarlier("none");
        feedRef.current = null;

        /* Changes the core sends before the attach answers wait, and those the answer already holds are dropped. */
        let mark: number | null = null;
        let early: { seq: number; changes: ChatChange[] }[] = [];
        let queued: ChatChange[] = [];
        let flushTimer: number | null = null;
        const flush = () => {
            if (flushTimer !== null) window.clearTimeout(flushTimer);
            flushTimer = null;
            if (queued.length === 0) return;
            const changes = queued;
            queued = [];
            setHeld((chat) => applyChanges(chat, changes));
        };
        flushRef.current = flush;

        const handleEvent = (event: AcpEvent) => {
            if (!mounted || event.agentId !== agent.id) return;
            if (event.kind === "reattach") {
                reattachingRef.current = true;
                setRestartKey((value) => value + 1);
                return;
            }
            if (event.kind === "stopped") {
                flush();
                setHeld((chat) => ({ ...chat, connection: "stopped", running: false }));
                return;
            }
            const seq = event.seq ?? 0;
            const changes = event.payload as ChatChange[];
            if (mark === null) {
                early.push({ seq, changes });
                return;
            }
            if (seq <= mark) return;
            queued.push(...changes);
            if (visibleRef.current) flush();
            else if (flushTimer === null) flushTimer = window.setTimeout(flush, HIDDEN_FLUSH_MS);
        };

        const lifecycle = lifecycleRef.current
            .catch(() => {})
            .then(async () => {
                if (!mounted) return;
                if (relaunch) await acpApi.stop(agent.id).catch(swallow("stop the chat agent"));
                await acpApi.subscribe(handleEvent, controller.signal);
                if (!mounted) return;
                const current = agentRef.current;
                let attached = await acpApi.attach(current.id);
                if (!mounted) return;
                if (attached.status !== "live") {
                    if (!startWantedRef.current) {
                        dormantRef.current = true;
                        mark = 0;
                        setHeld((chat) => ({ ...chat, connection: "ready", permissionMode: permissionModeOf(current) }));
                        return;
                    }
                    startWantedRef.current = false;
                    await workspaceSent();
                    await acpApi.startChat(current.id);
                    if (!mounted) return;
                    attached = await acpApi.attach(current.id);
                    if (!mounted) return;
                    if (attached.status !== "live") throw new Error("The chat stopped as soon as it started");
                }
                const { view, feed, seq } = attached;
                feedRef.current = feed;
                mark = seq;
                const caught = early.filter((batch) => batch.seq > seq).flatMap((batch) => batch.changes);
                early = [];
                setHeld(applyChanges(fromView(view), caught));
                setEarlier(view.olderBefore === undefined || view.olderBefore === null ? "none" : "idle");
            })
            .catch((error: unknown) => {
                if (controller.signal.aborted || !mounted) return;
                const message = errorText(error);
                setHeld((chat) => ({ ...chat, connection: "error", error: message }));
                setRecovery((recovering) => (recovering?.phase === "resuming" ? { phase: "failed", detail: message } : recovering));
            });

        lifecycleRef.current = lifecycle;
        return () => {
            mounted = false;
            if (flushTimer !== null) window.clearTimeout(flushTimer);
            flushRef.current = null;
            controller.abort();
            lifecycleRef.current = lifecycle;
        };
    }, [active, agent.id, agent.type, agent.modelProvider, cwd, restartKey]);

    useEffect(() => setSetMode(null), [held.sessionId]);
    const appliedMode = setMode ?? held.permissionMode;

    useEffect(() => {
        if (visible) flushRef.current?.();
    }, [visible]);

    /* The agent's permission mode follows the person's setting while the chat is up and shown. */
    useEffect(() => {
        const applied = appliedMode;
        if (!active || held.connection !== "ready" || changingPermissions || !applied || permissionMode === applied) return;
        setChangingPermissions(true);
        void acpApi
            .setPermissionMode(agent.id, permissionMode)
            .then(() => setSetMode(permissionMode))
            .catch((error: unknown) => {
                if (permissionModeOf(agentRef.current) === permissionMode)
                    cmd.setAgentPermissionMode(agent.id, applied as NonNullable<Agent["permissionMode"]>);
                onError(errorText(error));
            })
            .finally(() => setChangingPermissions(false));
    }, [active, agent.id, held.connection, appliedMode, permissionMode, changingPermissions, onError]);

    const loadEarlier = useCallback(() => {
        const feed = feedRef.current;
        const before = held.olderBefore;
        if (!feed || before === null || earlier === "loading") return;
        setEarlier("loading");
        acpApi.history(agent.id, feed, before, PAGE_TURNS).then(
            ({ page }) => {
                if (feedRef.current !== feed) return;
                setHeld((chat) => withPage(chat, page));
                setEarlier(page.olderBefore === undefined || page.olderBefore === null ? "none" : "idle");
            },
            () => {
                if (feedRef.current === feed) setEarlier("failed");
            },
        );
    }, [agent.id, earlier, held.olderBefore]);

    /** Shows a message this app is sending until the core shows it back; answers the id it goes out with. */
    const show = useCallback((text: string, paths: string[], context: PromptContext[], messageId: string = crypto.randomUUID()): string => {
        setPending((sending) => [...sending, { messageId, text, paths, context, at: Date.now() }]);
        return messageId;
    }, []);

    const unshow = useCallback((messageId: string) => {
        setPending((sending) => sending.filter((entry) => entry.messageId !== messageId));
    }, []);

    /** Takes the chat back to before `message` on this screen at once; the core says the same once it has. */
    const rewind = useCallback((message: ChatMessage) => {
        setHeld((chat) => applyChanges(chat, [{ op: "drop", from: message.id }]));
    }, []);

    const shownBack = useMemo(() => new Set(held.messages.flatMap((message) => (message.promptId ? [message.promptId] : []))), [held.messages]);
    useEffect(() => {
        if (pending.some((entry) => shownBack.has(entry.messageId)))
            setPending((sending) => sending.filter((entry) => !shownBack.has(entry.messageId)));
    }, [pending, shownBack]);

    const chat = useMemo((): ChatRows => {
        const waiting = pending.filter((entry) => !shownBack.has(entry.messageId));
        const error = held.error !== undefined && held.error === dismissed ? undefined : held.error;
        if (waiting.length === 0 && error === held.error) return held;
        return {
            ...held,
            error,
            failure: error === undefined ? undefined : held.failure,
            messages: waiting.length ? [...held.messages, ...waiting.map(pendingRow)] : held.messages,
            running: held.running || waiting.length > 0,
        };
    }, [held, pending, shownBack, dismissed]);

    const dismissError = useCallback(() => setDismissed(held.error ?? null), [held.error]);

    /** A message reached a chat whose agent was not running, so the core started it: take it up. */
    const woke = useCallback(() => {
        if (!dormantRef.current) return;
        dormantRef.current = false;
        reattachingRef.current = true;
        setRestartKey((value) => value + 1);
    }, []);

    return {
        chat,
        agentRef,
        recovery,
        retry,
        woke,
        changingPermissions,
        permissionMode,
        appliedMode,
        earlier,
        loadEarlier,
        show,
        unshow,
        rewind,
        dismissError,
    };
}
