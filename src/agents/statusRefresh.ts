import { getCurrentWindow } from "@tauri-apps/api/window";
import { agentApi } from "../api/agents";
import type { AgentType } from "../state/types";
import { invalidate } from "../state/resources";
import { swallow } from "../state/toast";

/** Coming back to the app often happens in bursts; checking every agent again once in this long is enough. */
export const REFRESH_AT_MOST_EVERY_MS = 10_000;

export interface StatusRefreshDeps {
    refresh: () => Promise<void>;
    invalidate: (matches: (kind: string) => boolean) => void;
    now: () => number;
}

const defaultDeps: StatusRefreshDeps = { refresh: () => agentApi.refreshStatuses(), invalidate, now: () => Date.now() };

/** Makes a refresher that forgets every agent's status and reloads the lists that show it, at most once per interval. */
export function statusRefresher(deps: StatusRefreshDeps = defaultDeps): () => Promise<void> {
    let last = -Infinity;
    return async () => {
        const now = deps.now();
        if (now - last < REFRESH_AT_MOST_EVERY_MS) return;
        last = now;
        await deps.refresh();
        deps.invalidate((kind) => kind === "agents.catalog" || kind === "agents.account");
    };
}

/** Checks every agent again when the person comes back to the app, since they may have signed in or out in a terminal. */
export function watchFocusForAgentStatus(refresh: () => Promise<void> = statusRefresher()): () => void {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const onFocus = () => void refresh().catch(swallow("refresh agent status"));
    const watchPage = () => window.addEventListener("focus", onFocus);
    try {
        getCurrentWindow()
            .onFocusChanged(({ payload }) => payload && onFocus())
            .then((stop) => (disposed ? stop() : (unlisten = stop)))
            .catch(watchPage);
    } catch {
        watchPage();
    }
    return () => {
        disposed = true;
        unlisten?.();
        window.removeEventListener("focus", onFocus);
    };
}

/** A chat refused for sign-in: the account is signed out now, whatever its CLI last said. */
export async function noteSignedOut(
    agent: AgentType,
    configPath: string | undefined,
    deps: Pick<StatusRefreshDeps, "invalidate"> & { mark: typeof agentApi.markSignedOut } = { invalidate, mark: agentApi.markSignedOut },
): Promise<void> {
    await deps.mark(agent, configPath);
    deps.invalidate((kind) => kind === "agents.catalog" || kind === "agents.account");
}
