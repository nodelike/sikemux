import type { CSSProperties, ReactNode } from "react";
import { IconWarning } from "../../../plugin-api/ui";
import type { JiraIssueSummary, JiraParent } from "../api";

function Glyph({ tone, title, children }: { tone: string; title: string; children: ReactNode }) {
    return (
        <svg
            className={`jira-glyph jira-tone-${tone}`}
            width={14}
            height={14}
            viewBox="0 0 16 16"
            fill="none"
            stroke="currentColor"
            strokeWidth={1.5}
            strokeLinecap="round"
            strokeLinejoin="round"
            role="img"
            aria-label={title}>
            <title>{title}</title>
            {children}
        </svg>
    );
}

const TYPES: Record<string, [string, ReactNode]> = {
    bug: [
        "danger",
        <>
            <rect x="2.5" y="2.5" width="11" height="11" rx="2.5" />
            <circle cx="8" cy="8" r="2.2" fill="currentColor" stroke="none" />
        </>,
    ],
    story: ["live", <path d="M4.5 2.5h7v11L8 10.8l-3.5 2.7Z" />],
    task: [
        "blue",
        <>
            <rect x="2.5" y="2.5" width="11" height="11" rx="2.5" />
            <path d="m5.4 8.1 1.8 1.8 3.5-3.6" />
        </>,
    ],
    subtask: [
        "blue",
        <>
            <rect x="2.5" y="2.5" width="6.5" height="6.5" rx="1.5" />
            <rect x="7" y="7" width="6.5" height="6.5" rx="1.5" />
        </>,
    ],
    epic: ["warn", <path d="M9 1.8 3.6 9h4l-1 5.2L12.4 7h-4Z" />],
};

/** An issue's type as the small coloured mark Jira puts beside its key. */
export function IssueTypeIcon({ type }: { type: string | null }) {
    if (!type) return null;
    const known = TYPES[type.toLowerCase().replace(/[^a-z]/g, "")];
    const [tone, shape] = known ?? [
        "muted",
        <>
            <rect x="2.5" y="2.5" width="11" height="11" rx="2.5" />
            <path d="M8 5.5v3M8 10.6v.1" />
        </>,
    ];
    return (
        <Glyph tone={tone} title={type}>
            {shape}
        </Glyph>
    );
}

const PRIORITIES: Record<string, [string, string]> = {
    highest: ["danger", "M4 8.5 8 4.5l4 4M4 12.5l4-4 4 4"],
    high: ["danger", "M4 10.5 8 6.5l4 4"],
    medium: ["warn", "M3.5 6.5h9M3.5 9.5h9"],
    low: ["blue", "M4 5.5 8 9.5l4-4"],
    lowest: ["blue", "M4 3.5 8 7.5l4-4M4 7.5l4 4 4-4"],
};

/** A priority as Jira draws it: chevrons up for urgent, bars for medium, chevrons down for low. */
export function PriorityGlyph({ priority }: { priority: string | null }) {
    if (!priority) return null;
    const [tone, path] = PRIORITIES[priority.toLowerCase()] ?? ["muted", "M3.5 8h9"];
    return (
        <Glyph tone={tone} title={`${priority} priority`}>
            <path d={path} />
        </Glyph>
    );
}

/** An issue's key, struck through once the issue is done, as Jira shows it. */
export function IssueKey({ issue, className = "jira-key" }: { issue: Pick<JiraIssueSummary, "key" | "statusCategory">; className?: string }) {
    return <span className={`${className}${issue.statusCategory === "done" ? " done" : ""}`}>{issue.key}</span>;
}

/** Jira's epic colours: the fourteen its boards number, then the ones newer projects name. */
const EPIC_COLORS: Record<string, string> = {
    color_1: "#a07150",
    color_2: "#f79232",
    color_3: "#d9a83a",
    color_4: "#3b7fc4",
    color_5: "#6a87a6",
    color_6: "#8eb021",
    color_7: "#ac707a",
    color_8: "#8b6bb8",
    color_9: "#f15c75",
    color_10: "#5d7aa8",
    color_11: "#2ab3c8",
    color_12: "#e5603e",
    color_13: "#3fb37e",
    color_14: "#7a69d6",
    purple: "#9f8fef",
    blue: "#579dff",
    green: "#4bce97",
    teal: "#60c6d2",
    yellow: "#e2b203",
    orange: "#fea362",
    grey: "#8c9bab",
    magenta: "#e774bb",
    dark_purple: "#6e5dc6",
    dark_blue: "#0c66e4",
    dark_green: "#1f845a",
    dark_teal: "#1d7f8c",
    dark_yellow: "#946f00",
    dark_orange: "#c25100",
    dark_grey: "#626f86",
    dark_magenta: "#ae4787",
};

export const epicColor = (color: string | null) => (color && EPIC_COLORS[color]) ?? "var(--gray-700)";

function CalendarIcon() {
    return (
        <svg width={12} height={12} viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round" aria-hidden="true">
            <rect x="2.5" y="3.5" width="11" height="10" rx="2" />
            <path d="M2.5 7h11M5.5 2v3M10.5 2v3" />
        </svg>
    );
}

/** The epic or parent an issue sits under, as a chip in the epic's colour. */
export function EpicChip({ parent }: { parent: JiraParent }) {
    return (
        <span className="jira-epic" style={{ "--epic": epicColor(parent.color) } as CSSProperties} title={`${parent.key} ${parent.summary}`}>
            <i />
            <span className="jira-ellipsis">{parent.summary || parent.key}</span>
        </span>
    );
}

/** A due date as Jira writes it, and whether it has passed with the issue still open. */
export function dueOf(due: string, done: boolean, today = new Date()): { label: string; late: boolean } {
    const [year, month, day] = due.split("-").map(Number);
    const date = new Date(year ?? 0, (month ?? 1) - 1, day ?? 1);
    const start = new Date(today.getFullYear(), today.getMonth(), today.getDate());
    return {
        label: date.toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" }),
        late: !done && date < start,
    };
}

/** A due date, turned red with a warning once it has passed and the issue is still open. */
export function DueChip({ due, done }: { due: string; done: boolean }) {
    const { label, late } = dueOf(due, done);
    return (
        <span className={`jira-due${late ? " late" : ""}`} title={late ? "Overdue" : "Due"}>
            {late ? <IconWarning size={12} /> : <CalendarIcon />}
            {label}
        </span>
    );
}
