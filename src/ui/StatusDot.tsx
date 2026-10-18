export type StatusTone = "ok" | "pending" | "idle" | "failed";

export function StatusDot({ tone, label }: { tone: StatusTone; label: string }) {
    return <span className={`status-dot ${tone}`} role="img" aria-label={label} />;
}
