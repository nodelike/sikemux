import { useEffect, useId, useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconSearch, Switch } from "../../../plugin-api/ui";
import type { Filter, FilterOp, Signal, TraceOrder } from "../api";
import { signozFieldKeysR, signozFieldValuesR } from "../resources";
import { SEVERITIES, addFilter, removeFilter, updateView, useExploreView } from "../state";

const TYPE_PAUSE_MS = 350;

export const OP_LABELS: Record<FilterOp, string> = {
    equals: "=",
    "not-equals": "≠",
    contains: "contains",
    "not-contains": "not contains",
    exists: "exists",
    "not-exists": "missing",
};

const takesValue = (op: FilterOp) => op !== "exists" && op !== "not-exists";

function useSettled<T>(value: T): T {
    const [settled, setSettled] = useState(value);
    useEffect(() => {
        const timer = window.setTimeout(() => setSettled(value), TYPE_PAUSE_MS);
        return () => window.clearTimeout(timer);
    }, [value]);
    return settled;
}

function FilterEditor({ signal, onAdd, onClose }: { signal: Signal; onAdd: (filter: Filter) => void; onClose: () => void }) {
    const [key, setKey] = useState("");
    const [op, setOp] = useState<FilterOp>("equals");
    const [value, setValue] = useState("");
    const keysId = useId();
    const valuesId = useId();
    const keyTyped = useSettled(key.trim());
    const valueTyped = useSettled(value.trim());
    const keys = useResourceEnabled(true, signozFieldKeysR, signal, keyTyped);
    const values = useResourceEnabled(!!keyTyped && takesValue(op), signozFieldValuesR, signal, keyTyped, valueTyped);
    const ready = !!key.trim() && (!takesValue(op) || !!value.trim());

    const add = () => {
        if (!ready) return;
        onAdd({ key: key.trim(), op, value: takesValue(op) ? value.trim() : "" });
        onClose();
    };
    const onKeyDown = (event: React.KeyboardEvent) => {
        if (event.key === "Enter") add();
        if (event.key === "Escape") onClose();
    };

    return (
        <div className="sgz-filter-editor" onKeyDown={onKeyDown}>
            <input
                className="sgz-input mono"
                placeholder="attribute"
                list={keysId}
                value={key}
                onChange={(event) => setKey(event.target.value)}
                autoFocus
                spellCheck={false}
                aria-label="Attribute"
            />
            <datalist id={keysId}>
                {(keys.data ?? []).map((field) => (
                    <option key={field.name} value={field.name}>
                        {field.dataType}
                    </option>
                ))}
            </datalist>
            <select className="sgz-input" value={op} onChange={(event) => setOp(event.target.value as FilterOp)} aria-label="Operator">
                {(Object.keys(OP_LABELS) as FilterOp[]).map((option) => (
                    <option key={option} value={option}>
                        {OP_LABELS[option]}
                    </option>
                ))}
            </select>
            {takesValue(op) && (
                <>
                    <input
                        className="sgz-input mono"
                        placeholder="value"
                        list={valuesId}
                        value={value}
                        onChange={(event) => setValue(event.target.value)}
                        spellCheck={false}
                        aria-label="Value"
                    />
                    <datalist id={valuesId}>
                        {(values.data ?? []).map((option) => (
                            <option key={option} value={option} />
                        ))}
                    </datalist>
                </>
            )}
            <button type="button" className="settings-btn primary" disabled={!ready} onClick={add}>
                Add
            </button>
            <button type="button" className="settings-btn" onClick={onClose}>
                Cancel
            </button>
        </div>
    );
}

export function FilterBar({ paneId, signal }: { paneId: string; signal: Signal }) {
    const view = useExploreView(paneId);
    const [adding, setAdding] = useState(false);
    const [showExpression, setShowExpression] = useState(view.expression !== "");
    const [text, setText] = useState(view.text);
    const [expression, setExpression] = useState(view.expression);
    const settledText = useSettled(text);

    useEffect(() => {
        if (settledText !== view.text) updateView(paneId, { text: settledText });
    }, [paneId, settledText, view.text]);

    const toggleSeverity = (severity: string) => {
        const on = view.severities.includes(severity);
        updateView(paneId, { severities: on ? view.severities.filter((item) => item !== severity) : [...view.severities, severity] });
    };

    return (
        <div className="sgz-filterbar">
            <div className="sgz-filter-row">
                {signal === "logs" && (
                    <>
                        <label className="sgz-search">
                            <IconSearch size={12} />
                            <input
                                className="sgz-input"
                                placeholder="Search log messages"
                                value={text}
                                onChange={(event) => setText(event.target.value)}
                                spellCheck={false}
                                aria-label="Search log messages"
                            />
                        </label>
                        <div className="sgz-severities" role="group" aria-label="Severity">
                            {SEVERITIES.map((severity) => (
                                <button
                                    key={severity}
                                    type="button"
                                    aria-pressed={view.severities.includes(severity)}
                                    className={`sgz-severity${view.severities.includes(severity) ? " on" : ""}`}
                                    onClick={() => toggleSeverity(severity)}>
                                    {severity}
                                </button>
                            ))}
                        </div>
                    </>
                )}
                {signal === "traces" && (
                    <>
                        <div className="sgz-segmented" role="group" aria-label="Order">
                            {(["slowest", "recent"] as TraceOrder[]).map((order) => (
                                <button
                                    key={order}
                                    type="button"
                                    aria-pressed={view.traceOrder === order}
                                    className={view.traceOrder === order ? "on" : ""}
                                    onClick={() => updateView(paneId, { traceOrder: order })}>
                                    {order}
                                </button>
                            ))}
                        </div>
                        <label className="sgz-toggle">
                            <Switch checked={view.tracesErrorsOnly} onChange={(tracesErrorsOnly) => updateView(paneId, { tracesErrorsOnly })} />
                            failed only
                        </label>
                    </>
                )}
                {view.filters.map((filter, index) => (
                    <span key={`${filter.key}:${filter.op}`} className="sgz-chip">
                        <span className="sgz-chip-key">{filter.key}</span>
                        <span className="sgz-chip-op">{OP_LABELS[filter.op]}</span>
                        {takesValue(filter.op) && <span className="sgz-chip-value">{filter.value}</span>}
                        <button
                            type="button"
                            className="sgz-chip-remove"
                            onClick={() => removeFilter(paneId, index)}
                            aria-label={`Remove ${filter.key} filter`}>
                            ×
                        </button>
                    </span>
                ))}
                {adding ? (
                    <FilterEditor signal={signal} onAdd={(filter) => addFilter(paneId, filter)} onClose={() => setAdding(false)} />
                ) : (
                    <button type="button" className="sgz-add-filter" onClick={() => setAdding(true)}>
                        + Filter
                    </button>
                )}
                <button
                    type="button"
                    className={`sgz-add-filter${showExpression ? " on" : ""}`}
                    aria-pressed={showExpression}
                    onClick={() => setShowExpression((shown) => !shown)}
                    title="Add a filter in SigNoz's own query syntax">
                    {"{ }"}
                </button>
            </div>
            {showExpression && (
                <input
                    className="sgz-input mono sgz-expression"
                    placeholder="latency_ms > 500 AND NOT (path = '/health')"
                    value={expression}
                    onChange={(event) => setExpression(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Enter") updateView(paneId, { expression });
                    }}
                    onBlur={() => updateView(paneId, { expression })}
                    spellCheck={false}
                    aria-label="SigNoz filter expression"
                />
            )}
        </div>
    );
}
