import { useEffect, useRef, type ReactNode } from "react";
import { formatAgo } from "@shared/lib/format";
import type { ResourceHandle } from "../state/resources";
import { IconSearch } from "@shared/ui/Icons";
import "./controls.css";

export interface SegOption<V extends string> {
    value: V;
    label: ReactNode;
    count?: number;
}

/** A row of choices of which one is on, such as a list's filters. */
export function Seg<V extends string>({
    options,
    value,
    onChange,
    label,
}: {
    options: readonly SegOption<V>[];
    value: V;
    onChange: (value: V) => void;
    /** What the choices pick, for people who cannot see the row. */
    label: string;
}) {
    return (
        <div className="plugin-seg" role="group" aria-label={label}>
            {options.map((option) => (
                <button
                    key={option.value}
                    type="button"
                    className={option.value === value ? "on" : ""}
                    aria-pressed={option.value === value}
                    onClick={() => onChange(option.value)}>
                    {option.label}
                    {option.count !== undefined && <span className="plugin-seg-count">{option.count}</span>}
                </button>
            ))}
        </div>
    );
}

function typing(target: EventTarget | null): boolean {
    return target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
}

/** A search box for a list. `/` anywhere in its pane jumps to it, and Escape clears it. */
export function Filter({ value, onChange, placeholder }: { value: string; onChange: (value: string) => void; placeholder: string }) {
    const input = useRef<HTMLInputElement>(null);

    useEffect(() => {
        const pane = input.current?.closest<HTMLElement>("[data-pane-id]");
        if (!pane) return;
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== "/" || event.metaKey || event.ctrlKey || event.altKey || typing(event.target)) return;
            event.preventDefault();
            input.current?.focus();
            input.current?.select();
        };
        pane.addEventListener("keydown", onKey);
        return () => pane.removeEventListener("keydown", onKey);
    }, []);

    return (
        <label className="plugin-filter">
            <IconSearch size={14} />
            <input
                ref={input}
                value={value}
                placeholder={placeholder}
                aria-label={placeholder}
                spellCheck={false}
                onChange={(event) => onChange(event.target.value)}
                onKeyDown={(event) => {
                    if (event.key === "Escape" && value) {
                        event.stopPropagation();
                        onChange("");
                    }
                }}
            />
            <kbd>/</kbd>
        </label>
    );
}

/** A line over data that could not be refreshed, saying how old it is. */
export function StaleNotice({ handle }: { handle: Pick<ResourceHandle<unknown>, "status" | "data" | "error" | "failure" | "loadedAt"> }) {
    if (handle.status !== "error" || handle.data === undefined) return null;
    const why = handle.failure === "offline" || handle.failure === "timeout" ? "Offline" : "Couldn't refresh";
    const age = handle.loadedAt === undefined ? "" : ` · showing data from ${formatAgo(handle.loadedAt)}`;
    return (
        <div className="plugin-stale" role="status" title={handle.error}>
            {why}
            {age}
        </div>
    );
}
