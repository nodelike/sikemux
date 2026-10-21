import type { Tone } from "../format";

/** A line of how many events came in over the period, scaled to its busiest point. */
export function Sparkline({ values, width = 72, height = 18 }: { values: number[]; width?: number; height?: number }) {
    if (values.length < 2) return <span className="sentry-spark-empty" aria-hidden="true" />;
    const most = Math.max(1, ...values);
    const step = width / (values.length - 1);
    const points = values.map((value, index) => `${(index * step).toFixed(1)},${(height - 1 - (value / most) * (height - 2)).toFixed(1)}`);
    return (
        <svg className="sentry-spark" width={width} height={height} viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
            <polyline points={points.join(" ")} fill="none" stroke="currentColor" strokeWidth="1.25" strokeLinejoin="round" />
        </svg>
    );
}

export function LevelMark({ tone, label }: { tone: Tone; label: string }) {
    return <span className={`sentry-level ${tone}`} role="img" aria-label={label} />;
}

export function initials(name: string): string {
    const words = name
        .trim()
        .split(/[\s@._-]+/)
        .filter(Boolean);
    return ((words[0]?.[0] ?? "") + (words[1]?.[0] ?? "")).toUpperCase() || "?";
}

export function Avatar({ name }: { name: string }) {
    return (
        <span className="sentry-avatar" title={`Assigned to ${name}`}>
            {initials(name)}
        </span>
    );
}

const SUBSTATUS: Record<string, string> = {
    new: "New",
    regressed: "Regressed",
    escalating: "Escalating",
};

export function Substatus({ value }: { value: string | null }) {
    const label = value ? SUBSTATUS[value] : undefined;
    return label ? <span className={`sentry-chip ${value}`}>{label}</span> : null;
}
