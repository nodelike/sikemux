import type { ReactNode } from "react";
import { IconCheck } from "./Icons";

/**
 * Shared form primitives.
 *
 * Every control here wraps a real native input so keyboard, form semantics and
 * screen readers keep working — the native box is hidden and a themed surface
 * is drawn from its `:checked` state instead of shipping the platform widget.
 */

/** On/off switch. Use for settings that take effect immediately. */
export function Switch({
    checked,
    onChange,
    disabled = false,
    label,
}: {
    checked: boolean;
    onChange: (value: boolean) => void;
    disabled?: boolean;
    /** Accessible name when the switch has no visible <label> around it. */
    label?: string;
}) {
    return (
        <span className={`swx${checked ? " on" : ""}${disabled ? " disabled" : ""}`}>
            <input
                type="checkbox"
                role="switch"
                checked={checked}
                disabled={disabled}
                aria-label={label}
                onChange={(event) => onChange(event.target.checked)}
            />
            <span className="swx-track" aria-hidden="true">
                <span className="swx-thumb" />
            </span>
        </span>
    );
}

/** Checkbox with a visible label. Use for multi-select sets, not for settings. */
export function Checkbox({
    checked,
    onChange,
    disabled = false,
    label,
    children,
}: {
    checked: boolean;
    onChange: (value: boolean) => void;
    disabled?: boolean;
    /** Accessible name when the box sits in a labelled column and shows no text. */
    label?: string;
    children?: ReactNode;
}) {
    return (
        <label className={`cbx${checked ? " on" : ""}${disabled ? " disabled" : ""}`}>
            <input
                type="checkbox"
                checked={checked}
                disabled={disabled}
                aria-label={children === undefined ? label : undefined}
                onChange={(event) => onChange(event.target.checked)}
            />
            <span className="cbx-box" aria-hidden="true">
                <IconCheck size={10} />
            </span>
            {children !== undefined && <span className="cbx-label">{children}</span>}
        </label>
    );
}

/** Range slider that shows its own value and fills the track up to the thumb. */
export function Slider({
    value,
    min,
    max,
    step = 1,
    onChange,
    label,
    format,
    disabled = false,
}: {
    value: number;
    min: number;
    max: number;
    step?: number;
    onChange: (value: number) => void;
    label?: string;
    /** Trailing readout, e.g. `(v) => \`${v}px\``. Omit when a separate field shows the value. */
    format?: (value: number) => string;
    disabled?: boolean;
}) {
    // CSS paints the filled portion from this ratio; no second element needed.
    const filled = max === min ? 0 : (value - min) / (max - min);
    return (
        <span className={`sld${disabled ? " disabled" : ""}`} style={{ ["--sld-fill" as string]: `${filled * 100}%` }}>
            <input
                type="range"
                min={min}
                max={max}
                step={step}
                value={value}
                disabled={disabled}
                aria-label={label}
                onChange={(event) => onChange(Number(event.target.value))}
            />
            {format && <span className="sld-value">{format(value)}</span>}
        </span>
    );
}
