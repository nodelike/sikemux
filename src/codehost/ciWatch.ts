import { useEffect } from "react";
import { coreCall } from "../api/core";
import { getIpcTransport } from "../api/transport";
import type { RepoRef } from "./types";
import { swallow } from "../state/toast";

const keyOf = (repo: Pick<RepoRef, "provider" | "owner" | "name">) => `${repo.provider}:${repo.owner}/${repo.name}`;

/** The repositories on screen, by how many places show each. */
const shown = new Map<string, { repo: RepoRef; count: number }>();
let sending = false;

/** Tells the core which repositories' CI to keep checking for this window: none while it is hidden. */
function send(): void {
    if (sending) return;
    sending = true;
    queueMicrotask(() => {
        sending = false;
        const repos = document.hidden
            ? []
            : [...shown.values()].map(({ repo }) => ({
                  provider: repo.provider,
                  owner: repo.owner,
                  name: repo.name,
                  account: repo.account ?? null,
                  host: repo.host ?? null,
              }));
        void coreCall("showCi", { repos }).catch(swallow("show CI"));
    });
}

let listening = false;

function listen(): void {
    if (listening) return;
    listening = true;
    document.addEventListener("visibilitychange", send);
    void getIpcTransport().subscribe("core_connected", send).catch(swallow("follow the core connecting"));
}

/** Keeps `repo`'s CI checked by the core while `on`; the core reads the lists that show it again whenever its runs move. */
export function useCiWatch(repo: RepoRef | null, on: boolean): void {
    const key = repo && on ? keyOf(repo) : null;
    const account = repo?.account ?? null;
    const host = repo?.host;
    useEffect(() => {
        if (!key || !repo) return;
        listen();
        const entry = shown.get(key);
        if (entry) entry.count += 1;
        else shown.set(key, { repo: { ...repo, account, host }, count: 1 });
        send();
        return () => {
            const current = shown.get(key);
            if (!current) return;
            current.count -= 1;
            if (current.count === 0) shown.delete(key);
            send();
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- the key, account and server name the repository; the object is new every render
    }, [key, account, host]);
}
