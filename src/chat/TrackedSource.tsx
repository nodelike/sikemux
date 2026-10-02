import { useEffect } from "react";
import { filterTracked, useTrackedItems, type TrackedItem } from "../codehost/tracked";
import type { RepoRef } from "../codehost/types";

export type TrackedMatches =
    { state: "loading" } | { state: "unavailable"; message: string } | { state: "ready"; repo: RepoRef; matches: TrackedItem[] };

/* Loaded only once someone types `#`, so reading a code host costs a chat
   nothing until it is asked for. */
export default function TrackedSource({
    cwd,
    needle,
    limit,
    onMatches,
}: {
    cwd: string;
    needle: string;
    limit: number;
    onMatches: (matches: TrackedMatches) => void;
}) {
    const list = useTrackedItems(cwd, true);
    useEffect(() => {
        onMatches(list.state === "ready" ? { state: "ready", repo: list.repo, matches: filterTracked(list.items, needle, limit) } : list);
    }, [limit, list, needle, onMatches]);
    return null;
}
