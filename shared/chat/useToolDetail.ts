import { useEffect, useState } from "react";
import { toolRunning } from "./toolLabels";
import type { ChatPart, ToolDetail } from "../wire.generated";

type ToolPart = Extract<ChatPart, { kind: "tool" }>;

/** What a tool's opened row fetched, and the error when the fetch failed; `detail` stays null when the host sends nothing. */
type FetchedDetail = { detail: ToolDetail | null; error: unknown };

/** Fetches what a call did once its row is `wanted`, and again as a running call goes on. */
export function useToolDetail(
    read: ((toolCallId: string, done: boolean) => Promise<ToolDetail | null>) | null,
    part: ToolPart,
    wanted = true,
): FetchedDetail {
    const [fetched, setFetched] = useState<FetchedDetail>({ detail: null, error: null });
    const done = !toolRunning(part.tool);
    const id = part.tool.toolCallId;
    const lines = part.output?.lines;
    useEffect(() => {
        if (!read || !wanted) return;
        let current = true;
        read(id, done).then(
            (detail) => {
                if (current) setFetched({ detail, error: null });
            },
            (error: unknown) => {
                if (current) setFetched((before) => ({ ...before, error }));
            },
        );
        return () => {
            current = false;
        };
    }, [read, wanted, id, done, lines]);
    return fetched;
}
