import { IconCheck, IconClock, IconClose, IconPullRequest, IconRun, IconStop, IconWarning } from "../../plugin-api/ui";
import { OUTCOME_LABEL, type Outcome } from "../runStatus";
import type { Section } from "../state";

const GLYPH: Record<Outcome, typeof IconCheck> = {
    running: IconRun,
    queued: IconClock,
    success: IconCheck,
    failure: IconClose,
    cancelled: IconStop,
    skipped: IconClock,
    blocked: IconWarning,
    unknown: IconWarning,
};

export function OutcomeIcon({ outcome, size = 13 }: { outcome: Outcome; size?: number }) {
    const Glyph = GLYPH[outcome];
    return (
        <span className="gha-outcome" data-outcome={outcome} title={OUTCOME_LABEL[outcome]} aria-label={OUTCOME_LABEL[outcome]} role="img">
            <Glyph size={size} />
        </span>
    );
}

const RING_MARK: Record<Outcome, React.ReactNode> = {
    running: <path d="M6.9 5.7 10.3 8l-3.4 2.3z" fill="currentColor" stroke="none" />,
    queued: <path d="M8 5.2V8l1.8 1.2" />,
    success: <path d="m5.4 8.2 1.8 1.8 3.5-3.7" />,
    failure: <path d="m5.9 5.9 4.2 4.2m0-4.2-4.2 4.2" />,
    cancelled: <path d="M5.6 10.4 10.4 5.6" />,
    skipped: <path d="M5.4 8h5.2" />,
    blocked: <path d="M8 5.2v3.3M8 10.6v.1" />,
    unknown: <path d="M8 5.2v3.3M8 10.6v.1" />,
};

/** How a run went, as Bitbucket draws it: the mark inside a ring. */
export function RingedOutcome({ outcome, size = 16 }: { outcome: Outcome; size?: number }) {
    return (
        <span className="gha-outcome" data-outcome={outcome} title={OUTCOME_LABEL[outcome]} aria-label={OUTCOME_LABEL[outcome]} role="img">
            <Stroke size={size}>
                <circle cx="8" cy="8" r="6.3" />
                {RING_MARK[outcome]}
            </Stroke>
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

export function UpDown({ size = 12 }: { size?: number }) {
    return (
        <Stroke size={size}>
            <path d="M5 6l3-3 3 3M5 10l3 3 3-3" />
        </Stroke>
    );
}

export function SignOutIcon({ size = 14 }: { size?: number }) {
    return (
        <Stroke size={size}>
            <path d="M6.5 2.5H4a1.5 1.5 0 0 0-1.5 1.5v8A1.5 1.5 0 0 0 4 13.5h2.5" />
            <path d="M10.5 5 13.5 8l-3 3M13.5 8H6.5" />
        </Stroke>
    );
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
