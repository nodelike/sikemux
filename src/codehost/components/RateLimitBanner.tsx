import { useEffect } from "react";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { useAccount, useHost } from "../registry";
import { rateLimitR } from "../resources";
import type { RateLimit } from "../types";
import { useNow } from "./hooks";

function clock(epochSecs: number): string {
    return new Date(epochSecs * 1000).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
}

export function waitInWords(ms: number): string {
    const secs = Math.max(1, Math.ceil(ms / 1000));
    if (secs < 60) return `${secs}s`;
    const minutes = Math.ceil(secs / 60);
    if (minutes < 60) return `${minutes} min`;
    return `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

/** What to tell the person about the limit, or nothing while there is plenty left. */
export function rateLimitNote(host: string, budget: RateLimit, now: number): { tone: "danger" | "warn"; text: string } | null {
    const resetsAt = budget.resetsAt;
    if (budget.limited && resetsAt !== null) {
        return {
            tone: "danger",
            text: `${host}'s rate limit is used up. Sikemux is holding requests until ${clock(resetsAt)}, in ${waitInWords(resetsAt * 1000 - now)}.`,
        };
    }
    if (!budget.near) return null;
    if (budget.remaining !== null && budget.limit !== null) {
        const resets = resetsAt === null ? "" : `, until ${clock(resetsAt)}`;
        return { tone: "warn", text: `${budget.remaining} of ${budget.limit} ${host} requests left this hour${resets}.` };
    }
    return { tone: "warn", text: `Close to ${host}'s hourly request limit.` };
}

/** Says when a host's rate limit is spent or nearly so, and reads everything again once it resets. */
export function RateLimitBanner({ active }: { active: boolean }) {
    const host = useHost();
    const budget = useResourceEnabled(active, rateLimitR, host.id, useAccount()).data;
    const limited = !!budget?.limited;
    const now = useNow(limited);
    const resetsAt = budget?.resetsAt ?? null;

    useEffect(() => {
        if (!limited || resetsAt === null) return;
        const timer = setTimeout(() => invalidate((kind) => kind.startsWith("host.")), Math.max(0, resetsAt * 1000 - Date.now()) + 1000);
        return () => clearTimeout(timer);
    }, [limited, resetsAt]);

    const note = budget ? rateLimitNote(host.name, budget, now) : null;
    if (!note) return null;
    return (
        <div className="gha-callout gha-rate-limit" data-tone={note.tone} role="status">
            {note.text}
        </div>
    );
}
