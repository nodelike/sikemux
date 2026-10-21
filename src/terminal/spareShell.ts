import { invokeCommand as invoke } from "../api/invoke";
import { coreSessionsApi } from "../api/coreSessions";
import { newId } from "../state/layout";
import { getState, useStore } from "../state/store";
import type { PtyContext, Session } from "../state/types";
import { noteSpawnedSession, offerResumableSessions } from "./sessionResume";
import { swallow } from "../state/toast";

/* A new tab takes over a shell started ahead of time with that tab's ids, so it
   never waits on the person's rc files. */

export interface SpareShell {
    readonly windowId: string;
    readonly paneId: string;
    readonly ptyId: number;
}

interface Pending {
    readonly key: string;
    readonly windowId: string;
    readonly paneId: string;
    ptyId: number | null;
}

const PREPARE_DELAY_MS = 1_000;

let spare: Pending | null = null;
let size: { cols: number; rows: number } | null = null;
let running = false;
let prepareTimer: number | undefined;

export function terminalContext(session: Session, windowId: string, paneId: string): PtyContext {
    return {
        sessionId: session.id,
        sessionName: session.name,
        sessionKind: session.kind,
        ...(session.kind === "project" && session.cwd ? { project: session.cwd } : {}),
        windowId,
        paneId,
        shellIntegration: session.kind === "project" || session.kind === "command",
    };
}

/** Spare shells start at the size terminals were last given; what an rc file prints at another size cannot be redrawn. */
export function noteTerminalSize(cols: number, rows: number): void {
    if (size?.cols === cols && size.rows === rows) return;
    size = { cols, rows };
    if (running) schedulePrepare();
}

function wantsSpare(session: Session | undefined): session is Session {
    return session?.kind === "project" || session?.kind === "command";
}

function spareKey(session: Session): string {
    return JSON.stringify([session.id, session.name, session.kind, session.cwd, size?.cols, size?.rows]);
}

function discard(): void {
    const ptyId = spare?.ptyId;
    spare = null;
    if (ptyId !== null && ptyId !== undefined) void coreSessionsApi.kill(ptyId).catch(swallow("kill a spare shell"));
}

function prepare(): void {
    const session = getState().sessions[getState().activeSessionId];
    if (!wantsSpare(session) || !size) {
        discard();
        return;
    }
    const key = spareKey(session);
    if (spare?.key === key) return;
    discard();
    const pending: Pending = { key, windowId: newId("win"), paneId: newId("pane"), ptyId: null };
    spare = pending;
    void invoke<number>("pty_spawn", {
        ...size,
        cwd: session.cwd || null,
        startup: null,
        directCommand: null,
        context: terminalContext(session, pending.windowId, pending.paneId),
    })
        .then((ptyId) => {
            noteSpawnedSession(ptyId);
            if (spare === pending) pending.ptyId = ptyId;
            else void coreSessionsApi.kill(ptyId).catch(swallow("kill a spare shell"));
        })
        .catch(() => {
            if (spare === pending) spare = null;
        });
}

function schedulePrepare(): void {
    window.clearTimeout(prepareTimer);
    prepareTimer = window.setTimeout(prepare, PREPARE_DELAY_MS);
}

/** The spare shell for `session`, if one has started; the next one starts shortly after. */
export function takeSpareShell(session: Session): SpareShell | null {
    schedulePrepare();
    if (!spare || spare.ptyId === null || spare.key !== spareKey(session)) return null;
    const taken = { windowId: spare.windowId, paneId: spare.paneId, ptyId: spare.ptyId };
    spare = null;
    offerResumableSessions([taken.ptyId]);
    return taken;
}

/** Keeps a spare shell ready for whichever project is in front. */
export function startSpareShells(): () => void {
    running = true;
    schedulePrepare();
    const unsubscribe = useStore.subscribe((state, previous) => {
        const session = state.sessions[state.activeSessionId];
        const before = previous.sessions[previous.activeSessionId];
        if (session === before) return;
        if (!session || !before || spareKey(session) !== spareKey(before)) schedulePrepare();
    });
    return () => {
        running = false;
        unsubscribe();
        window.clearTimeout(prepareTimer);
        discard();
    };
}
