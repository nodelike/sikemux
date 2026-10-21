import type { Tone } from "../format";

export function Dot({ tone, label }: { tone: Tone; label?: string }) {
    return <span className={`az-dot ${tone}`} role={label ? "img" : undefined} aria-label={label} aria-hidden={label ? undefined : true} />;
}

export function Tags({ tags }: { tags: [string, string][] }) {
    if (tags.length === 0) return null;
    return (
        <div className="az-tags">
            {tags.map(([key, value]) => (
                <span key={key} className="az-chip az-mono">
                    {key}={value}
                </span>
            ))}
        </div>
    );
}

export function Fact({ term, value, mono }: { term: string; value: string | null; mono?: boolean }) {
    return (
        <div>
            <dt>{term}</dt>
            <dd className={mono ? "az-mono" : undefined}>{value ?? "—"}</dd>
        </div>
    );
}
