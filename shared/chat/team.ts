import type { ChatMessage, TeamLine } from "../wire.generated";
import { english, type Say } from "../lib/say";
import { durationLabel } from "./durationLabel";

/** How far the reader opened a run of messages between agents. */
export type RunOpen = "closed" | "open" | "all";

export interface TeamRow {
    /** The run, by the row that names it. */
    id: string;
    /** What its line says, on the first row of it held. */
    head: TeamLine | null;
    /** The row draws nothing besides `head`. */
    hidden: boolean;
    /** Which parts stay in view, or null when the row shows whole. */
    shown: readonly number[] | null;
}

/**
 * How each row of a run shows, by the row. A run whose line is not held, as when rows after it
 * were dropped, shows whole, as nothing could open it.
 */
export function teamRows(rows: readonly ChatMessage[], how: (id: string) => RunOpen): Map<string, TeamRow> {
    const found = new Map<string, TeamRow>();
    let at = 0;
    while (at < rows.length) {
        const id = rows[at].team?.id;
        if (id === undefined) {
            at += 1;
            continue;
        }
        let end = at;
        let line: TeamLine | undefined;
        while (end < rows.length && rows[end].team?.id === id) {
            line = rows[end].team?.line ?? line;
            end += 1;
        }
        const open = line ? how(id) : "all";
        const from = open === "open" && line ? rows.findIndex((row, index) => index >= at && index < end && row.id === line.from) : -1;
        for (let index = at; index < end; index += 1) {
            const kept = rows[index].team?.shown ?? [];
            found.set(rows[index].id, {
                id,
                head: index === at ? (line ?? null) : null,
                hidden: open === "closed" ? kept.length === 0 : index < from,
                shown: open === "closed" ? kept : null,
            });
        }
        at = end;
    }
    return found;
}

/** What a run's line says after its count: who wrote last, and their first words. */
export function teamWords({ latest }: TeamLine): string {
    return latest ? `@${latest.name}: ${latest.text}` : "";
}

export function teamCount(messages: number, say: Say = english): string {
    return messages === 1 ? say("1 message") : say("{count} messages", { count: messages });
}

/** How long a run lasted, to the minute once it passes one: `41m`, `2h 6m`. */
export function teamTook(ms: number, say: Say = english): string {
    if (ms < 60_000) return durationLabel(ms, say);
    const minutes = Math.round(ms / 60_000);
    return minutes < 60
        ? say("{count}m", { count: minutes })
        : say("{hours}h {minutes}m", { hours: Math.floor(minutes / 60), minutes: minutes % 60 });
}
