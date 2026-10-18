import { useSyncExternalStore } from "react";
import { coreSessionsApi, type SshLink } from "../api/coreSessions";
import { getIpcTransport } from "../api/transport";
import { collectPanes } from "../state/layout";
import { useStore, type StoreState } from "../state/store";
import { swallow } from "../state/toast";
import type { StatusTone } from "../ui/StatusDot";

type LinkState = Exclude<SshLink, "disconnected"> | "failed";

/** Only terminals whose SSH is up, on its way, or gave up; any other terminal counts as not connected. */
const live = new Map<number, LinkState>();
const listeners = new Set<() => void>();

/* The loop says "disconnected" both after a logout and after giving up. A logout follows "connected";
   giving up follows a "connecting" that never got through. */
function next(previous: LinkState | undefined, link: SshLink): LinkState | undefined {
    if (link !== "disconnected") return link;
    return previous === "connecting" ? "failed" : undefined;
}

function note(ptyId: number, link: SshLink): void {
    const state = next(live.get(ptyId), link);
    if (live.get(ptyId) === state) return;
    if (state === undefined) live.delete(ptyId);
    else live.set(ptyId, state);
    for (const listener of [...listeners]) listener();
}

function forget(ptyId: number): void {
    if (live.delete(ptyId)) for (const listener of [...listeners]) listener();
}

/** Follows every SSH terminal for as long as the page runs, shown or not. */
export function watchSshLinks(): () => void {
    const controller = new AbortController();
    const { signal } = controller;
    const transport = getIpcTransport();
    const fail = (error: unknown) => {
        if (!signal.aborted) swallow("ssh link listener")(error);
    };
    void transport
        .subscribe<{ id: number; link: SshLink }>("pty_ssh_link", (event) => note(event.payload.id, event.payload.link), { signal })
        .catch(fail);
    void transport.subscribe<{ id: number }>("pty_exited", (event) => forget(event.payload.id), { signal }).catch(fail);
    void coreSessionsApi
        .list()
        .then((sessions) => {
            for (const session of sessions)
                if (session.ssh && session.running && !live.has(session.id) && !signal.aborted) note(session.id, session.ssh);
        })
        .catch(fail);
    return () => controller.abort();
}

function subscribe(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** The session's terminals, as one string so a store selector over it stays equal while they do. */
function sessionPtyIds(state: StoreState, sessionId: string): string {
    const ids: number[] = [];
    for (const windowId of state.windowsBySession[sessionId] ?? []) {
        const window = state.windows[windowId];
        if (window) for (const pane of collectPanes(window.root)) if (pane.ptyId !== undefined) ids.push(pane.ptyId);
    }
    return ids.join(",");
}

function toneOf(ptyIds: string): StatusTone {
    const links = ptyIds ? ptyIds.split(",").map((id) => live.get(Number(id))) : [];
    if (links.includes("connected")) return "ok";
    if (links.includes("connecting")) return "pending";
    if (links.includes("failed")) return "failed";
    return "idle";
}

export function useSshTone(sessionId: string): StatusTone {
    const ptyIds = useStore((state) => sessionPtyIds(state, sessionId));
    return useSyncExternalStore(subscribe, () => toneOf(ptyIds));
}
