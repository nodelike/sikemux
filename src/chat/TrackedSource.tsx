import { useEffect } from "react";
import { filterTracked, useTrackedItems, type TrackedItem, type TrackedKind } from "../codehost/tracked";
import type { RepoRef } from "../codehost/types";

export type TrackedMatches =
    { state: "loading" } | { state: "unavailable"; message: string } | { state: "ready"; repo: RepoRef; matches: TrackedItem[] };

/* Loaded only once someone types `#` or `!`, so reading a code host costs a chat
   nothing until it is asked for. */
export default function TrackedSource({
    cwd,
    kind,
    needle,
    limit,
    onMatches,
}: {
    cwd: string;
    kind: TrackedKind;
    needle: string;
    limit: number;
    onMatches: (matches: TrackedMatches) => void;
}) {
    const list = useTrackedItems(cwd, true);
    useEffect(() => {
        onMatches(
            list.state === "ready"
                ? {
                      state: "ready",
                      repo: list.repo,
                      matches: filterTracked(
                          list.items.filter((item) => item.kind === kind),
                          needle,
                          limit,
                      ),
                  }
                : list,
        );
    }, [kind, limit, list, needle, onMatches]);
    return null;
}
