import { coreSessionsApi } from "../api/coreSessions";
import { externalValue, useExternalValue } from "@shared/lib/externalValue";
import type { SshLink } from "@shared/wire.generated";
import { getIpcTransport } from "../api/transport";
import { collectPanes } from "../state/layout";
import { useStore, type StoreState } from "../state/store";
import { swallow } from "../state/toast";
import type { StatusTone } from "../ui/StatusDot";

type LinkState = Exclude<SshLink, "disconnected"> | "failed";

/** Only terminals whose SSH is up, on its way, or gave up; any other terminal counts as not connected. */
const links = externalValue<ReadonlyMap<number, LinkState>>(new Map());

/* The loop says "disconnected" both after a logout and after giving up. A logout follows "connected";
   giving up follows a "connecting" that never got through. */
function next(previous: LinkState | undefined, link: SshLink): LinkState | undefined {
    if (link !== "disconnected") return link;
    return previous === "connecting" ? "failed" : undefined;
}

function note(ptyId: number, link: SshLink): void {
    const live = links.get();
    const state = next(live.get(ptyId), link);
    if (live.get(ptyId) === state) return;
    const updated = new Map(live);
    if (state === undefined) updated.delete(ptyId);
    else updated.set(ptyId, state);
    links.set(updated);
}

function forget(ptyId: number): void {
    if (!links.get().has(ptyId)) return;
    const updated = new Map(links.get());
    updated.delete(ptyId);
    links.set(updated);
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
                if (session.ssh && session.running && !links.get().has(session.id) && !signal.aborted) note(session.id, session.ssh);
        })
        .catch(fail);
    return () => controller.abort();
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

function toneOf(live: ReadonlyMap<number, LinkState>, ptyIds: string): StatusTone {
    const links = ptyIds ? ptyIds.split(",").map((id) => live.get(Number(id))) : [];
    if (links.includes("connected")) return "ok";
    if (links.includes("connecting")) return "pending";
    if (links.includes("failed")) return "failed";
    return "idle";
}

export function useSshTone(sessionId: string): StatusTone {
    const ptyIds = useStore((state) => sessionPtyIds(state, sessionId));
    return useExternalValue(links, (live) => toneOf(live, ptyIds));
}
