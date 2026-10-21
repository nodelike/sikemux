import { Switch } from "../../../plugin-api/ui";
import type { Tone } from "../format";

export function Dot({ tone, label }: { tone: Tone; label?: string }) {
    return <span className={`k8s-dot ${tone}`} role={label ? "img" : undefined} aria-label={label} aria-hidden={label ? undefined : true} />;
}

/** How much of something is ready, as a number and a thin bar. */
export function ReadyBar({ ready, of }: { ready: number; of: number }) {
    const share = of === 0 ? 100 : Math.min(100, Math.round((ready / of) * 100));
    return (
        <span className="k8s-ready">
            <span className="k8s-ready-count">
                {ready}/{of}
            </span>
            <span className={`k8s-ready-bar${ready < of ? " short" : ""}`} aria-hidden="true">
                <span style={{ width: `${share}%` }} />
            </span>
        </span>
    );
}

export function Toggle({
    label,
    checked,
    disabled,
    onChange,
}: {
    label: string;
    checked: boolean;
    disabled?: boolean;
    onChange: (value: boolean) => void;
}) {
    return (
        <label className={`k8s-toggle${disabled ? " disabled" : ""}`}>
            <Switch checked={checked} disabled={disabled} onChange={onChange} />
            <span>{label}</span>
        </label>
    );
}
