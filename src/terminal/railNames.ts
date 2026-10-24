import { coreSessionsApi } from "../api/coreSessions";
import { getIpcTransport } from "../api/transport";
import { externalValue, useExternalValue } from "../lib/externalValue";
import { collectPanes } from "../state/layout";
import { useStore, type StoreState } from "../state/store";
import { swallow } from "../state/toast";
import type { Session } from "../state/types";

/** What the core calls each terminal outside every project, agent and SSH host, by its id. */
const names = externalValue<ReadonlyMap<number, string>>(new Map());

let reading: Promise<void> | null = null;

/** Reads the names the core gave its terminals, once however many ask at the same time. */
export function readRailNames(): Promise<void> {
    reading ??= coreSessionsApi
        .list()
        .then((sessions) => {
            const next = new Map(names.get());
            for (const session of sessions) if (session.railName && session.running) next.set(session.id, session.railName);
            names.set(next);
        })
        .catch(swallow("read terminal names"))
        .finally(() => (reading = null));
    return reading;
}

/** Keeps the core's terminal names for as long as the page runs. */
export function watchRailNames(): () => void {
    const controller = new AbortController();
    void getIpcTransport()
        .subscribe<{ id: number }>(
            "pty_exited",
            ({ payload }) => {
                if (!names.get().has(payload.id)) return;
                const next = new Map(names.get());
                next.delete(payload.id);
                names.set(next);
            },
            { signal: controller.signal },
        )
        .catch(swallow("follow terminal names"));
    void readRailNames();
    return () => controller.abort();
}

function firstPtyId(state: Pick<StoreState, "windowsBySession" | "windows">, sessionId: string): number | undefined {
    for (const windowId of state.windowsBySession[sessionId] ?? []) {
        const window = state.windows[windowId];
        const pane = window && collectPanes(window.root).find((candidate) => candidate.ptyId !== undefined);
        if (pane) return pane.ptyId;
    }
    return undefined;
}

/** A session's name as the person reads it: an unnamed terminal session's is the one the core gave its first terminal. */
export function sessionName(
    state: Pick<StoreState, "windowsBySession" | "windows">,
    session: Session,
    railNames: ReadonlyMap<number, string> = names.get(),
): string {
    if (session.kind !== "command") return session.name;
    const ptyId = firstPtyId(state, session.id);
    return (ptyId !== undefined && railNames.get(ptyId)) || session.name || "Terminal";
}

export function useSessionName(session: Session | undefined): string {
    const ptyId = useStore((state) => (session?.kind === "command" ? firstPtyId(state, session.id) : undefined));
    const railName = useExternalValue(names, (live) => (ptyId === undefined ? undefined : live.get(ptyId)));
    if (session?.kind !== "command") return session?.name ?? "";
    return railName ?? (session.name || "Terminal");
}

/** Every name the core gave, for lists that name many sessions at once. */
export function useRailNames(): ReadonlyMap<number, string> {
    return useExternalValue(names);
}
