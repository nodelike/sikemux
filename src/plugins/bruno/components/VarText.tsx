// Inputs/textareas that color-code {{variables}}. Native form fields can't style
// substrings, so we render a mirrored, syntax-highlighted backdrop stacked under
// a transparent field (caret + selection show through). Known vars render in the
// accent colour; vars missing from the active scope render red, like Bruno.

import { useRef } from "react";
import type { Scope } from "../lib/interpolate";

function tokens(text: string, scope: Scope): React.ReactNode[] {
    const out: React.ReactNode[] = [];
    const re = /\{\{\s*([^}]+?)\s*\}\}/g;
    let last = 0;
    let i = 0;
    let m: RegExpExecArray | null;
    while ((m = re.exec(text))) {
        if (m.index > last) out.push(text.slice(last, m.index));
        const key = m[1].trim();
        const known = key.startsWith("process.env.") || key in scope;
        out.push(
            <span key={i++} className={`bruno-var${known ? "" : " missing"}`}>
                {m[0]}
            </span>,
        );
        last = m.index + m[0].length;
    }
    // trailing zero-width char keeps the final (possibly empty) line laid out
    out.push(text.slice(last) + "​");
    return out;
}

interface InputProps {
    value: string;
    scope: Scope;
    onChange: (v: string) => void;
    onKeyDown?: (e: React.KeyboardEvent<HTMLInputElement>) => void;
    placeholder?: string;
    className?: string;
}

export function VarInput({ value, scope, onChange, onKeyDown, placeholder, className }: InputProps) {
    const back = useRef<HTMLDivElement>(null);
    return (
        <div className={`bruno-varwrap${className ? ` ${className}` : ""}`}>
            <div className="bruno-var-back oneline" ref={back} aria-hidden="true">
                {tokens(value, scope)}
            </div>
            <input
                className="bruno-var-field"
                value={value}
                placeholder={placeholder}
                spellCheck={false}
                onChange={(e) => onChange(e.target.value)}
                onKeyDown={onKeyDown}
                onScroll={(e) => {
                    if (back.current) back.current.scrollLeft = e.currentTarget.scrollLeft;
                }}
            />
        </div>
    );
}

interface AreaProps {
    value: string;
    scope: Scope;
    onChange: (v: string) => void;
    placeholder?: string;
    className?: string;
}

export function VarArea({ value, scope, onChange, placeholder, className }: AreaProps) {
    const back = useRef<HTMLDivElement>(null);
    return (
        <div className={`bruno-varwrap area${className ? ` ${className}` : ""}`}>
            <div className="bruno-var-back" ref={back} aria-hidden="true">
                {tokens(value, scope)}
            </div>
            <textarea
                className="bruno-var-field area"
                value={value}
                placeholder={placeholder}
                spellCheck={false}
                onChange={(e) => onChange(e.target.value)}
                onScroll={(e) => {
                    if (back.current) back.current.scrollTop = e.currentTarget.scrollTop;
                }}
            />
        </div>
    );
}
