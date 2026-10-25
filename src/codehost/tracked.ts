import { useMemo } from "react";
import { useResourceEnabled } from "../plugin-api/resources";
import { errorText } from "@shared/lib/format";
import { useHostRepo } from "./project";
import { codeHost } from "./registry";
import { hostStatusR } from "./resources";
import { trackedListR } from "./lists";
import type { RepoRef, TrackedItem } from "./types";

export type { TrackedItem, TrackedKind } from "./types";
export { loadTrackedContext } from "./lists";

/** Digits match the start of a number; words must all appear in the title. */
export function filterTracked(items: readonly TrackedItem[], needle: string, limit: number): TrackedItem[] {
    const query = needle.trim().toLowerCase();
    if (/^\d+$/.test(query)) return items.filter((item) => String(item.number).startsWith(query)).slice(0, limit);
    const words = query.split(/\s+/).filter(Boolean);
    return items.filter((item) => words.every((word) => item.title.toLowerCase().includes(word))).slice(0, limit);
}

export type TrackedList = { state: "loading" } | { state: "unavailable"; message: string } | { state: "ready"; repo: RepoRef; items: TrackedItem[] };

/** The open issues and pull requests of the repository a project's remote points at, once someone is signed in to its host. */
export function useTrackedItems(cwd: string, enabled: boolean): TrackedList {
    const found = useHostRepo(cwd || null, enabled && !!cwd);
    const repo = found.repo;
    const host = repo ? codeHost(repo.provider) : undefined;
    const status = useResourceEnabled(enabled && !!repo, hostStatusR, repo?.provider ?? "", repo?.account ?? null);
    const signedIn = !!status.data?.ok;
    const items = useResourceEnabled(enabled && signedIn && !!cwd, trackedListR, cwd);
    return useMemo((): TrackedList => {
        if (!cwd) return { state: "unavailable", message: "Open a project to pick its issues" };
        if (found.loading) return { state: "loading" };
        if (!repo || !host) return { state: "unavailable", message: "This project has no remote on a code host" };
        if (!status.data) return status.error ? { state: "unavailable", message: errorText(status.error) } : { state: "loading" };
        if (!signedIn) return { state: "unavailable", message: `Sign in to ${host.name} in the Git pane to pick from its issues` };
        if (items.data) return { state: "ready", repo, items: items.data };
        if (items.error) return { state: "unavailable", message: errorText(items.error) };
        return { state: "loading" };
    }, [cwd, found.loading, repo, host, status.data, status.error, signedIn, items.data, items.error]);
}
