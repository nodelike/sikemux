import { IconPullRequest } from "../../plugin-api/ui";
import { OUTCOME_LABEL, type Outcome } from "../runStatus";
import type { Section } from "../state";
import "../styles/marks.css";

/** An outcome, or a job a run will make but has not started yet. */
export type Mark = Outcome | "idle";

/** The tick, cross or slash drawn over an ended outcome's circle, on a 16 grid. */
export const MARK_INNER: Partial<Record<Mark, string>> = {
    success: "m5.2 8.2 1.9 1.9 3.7-3.9",
    failure: "m5.8 5.8 4.4 4.4m0-4.4-4.4 4.4",
    cancelled: "M5.6 10.4 10.4 5.6",
    skipped: "M5.6 10.4 10.4 5.6",
};

/**
 * How a run, job, step or check is going, as GitHub draws it: a filled circle with its tick, cross or slash once it
 * ends, an amber dot while it waits, an amber ring spinning while it runs, and an empty circle before it starts.
 */
export function StatusMark({ outcome, size = 14 }: { outcome: Mark; size?: number }) {
    const label = outcome === "idle" ? "Not started" : OUTCOME_LABEL[outcome];
    const inner = MARK_INNER[outcome];
    const style = { "--mark": `${size}px` } as React.CSSProperties;
    if (outcome === "running" || outcome === "queued" || outcome === "blocked")
        return (
            <span className="status-mark" data-outcome={outcome} style={style} role="img" aria-label={label} title={label}>
                <span className="pr-ci-pending" data-outcome={outcome === "running" ? "running" : "queued"} />
            </span>
        );
    return (
        <span className="status-mark" data-outcome={outcome} style={style} role="img" aria-label={label} title={label}>
            <svg
                width={size}
                height={size}
                viewBox="0 0 16 16"
                fill="none"
                strokeWidth={1.6}
                strokeLinecap="round"
                strokeLinejoin="round"
                aria-hidden="true">
                {inner ? (
                    <circle cx="8" cy="8" r="7.2" fill="currentColor" />
                ) : (
                    <circle cx="8" cy="8" r="6.4" stroke="currentColor" strokeWidth={1.3} />
                )}
                {inner && <path d={inner} className="status-mark-inner" />}
            </svg>
        </span>
    );
}

export function MoreDots({ size = 13 }: { size?: number }) {
    return (
        <svg width={size} height={size} viewBox="0 0 16 16" fill="currentColor" aria-hidden="true">
            <circle cx="3" cy="8" r="1.4" />
            <circle cx="8" cy="8" r="1.4" />
            <circle cx="13" cy="8" r="1.4" />
        </svg>
    );
}

function Stroke({ size, children }: { size: number; children: React.ReactNode }) {
    return (
        <svg
            width={size}
            height={size}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth={1.4}
            strokeLinecap="round"
            strokeLinejoin="round"
            aria-hidden="true">
            {children}
        </svg>
    );
}

export function SectionIcon({ section, size = 14 }: { section: Section; size?: number }) {
    switch (section) {
        case "actions":
            return (
                <Stroke size={size}>
                    <circle cx="8" cy="8" r="6" />
                    <path d="M6.7 5.6 10.3 8l-3.6 2.4z" />
                </Stroke>
            );
        case "pulls":
            return <IconPullRequest size={size} />;
        case "issues":
            return (
                <Stroke size={size}>
                    <circle cx="8" cy="8" r="6" />
                    <circle cx="8" cy="8" r="1.2" fill="currentColor" stroke="none" />
                </Stroke>
            );
        case "releases":
            return (
                <Stroke size={size}>
                    <path d="M2.5 3.5v3.4c0 .4.2.8.4 1l5.2 5.2a1.2 1.2 0 0 0 1.7 0l3.3-3.3a1.2 1.2 0 0 0 0-1.7L7.9 2.9a1.4 1.4 0 0 0-1-.4H3.5a1 1 0 0 0-1 1Z" />
                    <circle cx="5.3" cy="5.3" r=".8" fill="currentColor" stroke="none" />
                </Stroke>
            );
        case "inbox":
            return (
                <Stroke size={size}>
                    <path d="M2.2 8.8 4 3.4c.2-.5.6-.9 1.2-.9h5.6c.6 0 1 .4 1.2.9l1.8 5.4" />
                    <path d="M2.2 8.8v3.4c0 .7.6 1.3 1.3 1.3h9c.7 0 1.3-.6 1.3-1.3V8.8h-3.2L9.6 10.4H6.4L5.4 8.8Z" />
                </Stroke>
            );
    }
}

export function NotPlannedIcon({ size = 14 }: { size?: number }) {
    return (
        <Stroke size={size}>
            <circle cx="8" cy="8" r="6" />
            <path d="M3.8 12.2 12.2 3.8" />
        </Stroke>
    );
}

export function CommentIcon({ size = 12 }: { size?: number }) {
    return (
        <Stroke size={size}>
            <path d="M3 3.5h10a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H8l-3 2.5v-2.5H3a1 1 0 0 1-1-1v-6a1 1 0 0 1 1-1Z" />
        </Stroke>
    );
}
