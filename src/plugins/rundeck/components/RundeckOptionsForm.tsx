import { useEffect, useId, useMemo, useRef, type ReactNode } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { Checkbox, Dropdown } from "../../../plugin-api/ui";
import { rndOptionValuesV } from "../resources";
import type { RundeckJobOption as JobOption, RundeckOptionChoice as OptionValue } from "../types.generated";
import { splitMulti } from "./options";

interface Props {
    paneId: string;
    options: JobOption[];
    values: Record<string, string>;
    /** Each option's values URL with the other options filled in. */
    valuesUrls: Record<string, string>;
    errors: Record<string, string>;
    showErrors: boolean;
    permalink: string | null;
    active: boolean;
    onChange: (name: string, value: string) => void;
    onRemoteValues: (name: string, values: string[] | null) => void;
    hint?: (option: JobOption) => ReactNode;
}

export function RundeckOptionsForm({
    paneId,
    options,
    values,
    valuesUrls,
    errors,
    showErrors,
    permalink,
    active,
    onChange,
    onRemoteValues,
    hint,
}: Props) {
    if (options.length === 0) return <div className="rnd-plan-row muted">This job takes no options.</div>;
    return (
        <div className="rnd-options">
            {options.map((option) => (
                <OptionField
                    key={option.name}
                    slot={`${paneId}/values/${option.name}`}
                    option={option}
                    value={values[option.name] ?? ""}
                    valuesUrl={valuesUrls[option.name] ?? ""}
                    error={showErrors ? errors[option.name] : undefined}
                    permalink={permalink}
                    active={active}
                    onChange={(value) => onChange(option.name, value)}
                    onRemoteValues={(list) => onRemoteValues(option.name, list)}
                    hint={hint?.(option)}
                />
            ))}
        </div>
    );
}

interface FieldProps {
    slot: string;
    option: JobOption;
    value: string;
    valuesUrl: string;
    error: string | undefined;
    permalink: string | null;
    active: boolean;
    onChange: (value: string) => void;
    onRemoteValues: (values: string[] | null) => void;
    hint: ReactNode;
}

function OptionField({ slot, option, value, valuesUrl, error, permalink, active, onChange, onRemoteValues, hint }: FieldProps) {
    const id = useId();
    const remote = useResourceEnabled(active && !!valuesUrl, rndOptionValuesV, slot, valuesUrl);
    const remoteFailed = !!option.valuesUrl && remote.status === "error";
    const report = useRef(onRemoteValues);
    report.current = onRemoteValues;

    useEffect(() => {
        if (!option.valuesUrl) return;
        if (remote.data) report.current(remote.data.map((entry) => entry.value));
        else if (remote.status === "error") report.current(null);
    }, [remote.data, remote.status, option.valuesUrl]);

    const choices = useMemo<OptionValue[] | null>(() => {
        if (option.valuesUrl) return remote.data ?? null;
        return option.values?.length ? option.values.map((entry) => ({ name: entry, value: entry })) : null;
    }, [option.valuesUrl, option.values, remote.data]);

    const label = option.label || option.name;

    return (
        <div className={`rnd-field rnd-option${error ? " invalid" : ""}`}>
            <span className="rnd-option-label">
                <label htmlFor={id}>{label}</label>
                {option.required && (
                    <span className="rnd-option-req" aria-label="required">
                        *
                    </span>
                )}
                {option.label && option.label !== option.name && <span className="rnd-option-key">{option.name}</span>}
            </span>
            <OptionInput id={id} option={option} value={value} choices={choices} permalink={permalink} onChange={onChange} />
            {hint}
            {option.description && <small className="rnd-field-help">{option.description}</small>}
            {option.valuesUrl && remote.status === "loading" && !remote.data && <small className="rnd-field-help">loading values…</small>}
            {remoteFailed && <small className="rnd-field-help warn">Couldn't load the allowed values ({remote.error}). Type a value instead.</small>}
            {error && <small className="rnd-field-error">{error}</small>}
        </div>
    );
}

function OptionInput({
    id,
    option,
    value,
    choices,
    permalink,
    onChange,
}: {
    id: string;
    option: JobOption;
    value: string;
    choices: OptionValue[] | null;
    permalink: string | null;
    onChange: (value: string) => void;
}) {
    const listId = `${id}-values`;
    const textProps = { spellCheck: false, autoCapitalize: "off", autoCorrect: "off" } as const;

    if (option.kind === "file") {
        return (
            <span className="rnd-option-file">
                <input id={id} type="text" disabled value="" placeholder="file options must be run from Rundeck" />
                {permalink && (
                    <button type="button" className="rnd-field-link" onClick={() => void openUrl(permalink).catch(swallow("open Rundeck URL"))}>
                        open in Rundeck ↗
                    </button>
                )}
            </span>
        );
    }

    if (option.secure) {
        return <input id={id} type="password" autoComplete="new-password" value={value} onChange={(e) => onChange(e.target.value)} />;
    }

    if (option.enforced && choices) {
        if (option.multivalued) {
            const delimiter = option.delimiter;
            const picked = new Set(splitMulti(value, delimiter));
            const toggle = (entry: string, on: boolean) => {
                const next = choices.map((c) => c.value).filter((v) => (v === entry ? on : picked.has(v)));
                onChange(next.join(delimiter));
            };
            return (
                <span className="rnd-option-multi" id={id} role="group">
                    {choices.map((choice) => (
                        <Checkbox key={choice.value} checked={picked.has(choice.value)} onChange={(on) => toggle(choice.value, on)}>
                            {choice.name}
                        </Checkbox>
                    ))}
                </span>
            );
        }
        const known = choices.some((c) => c.value === value);
        return (
            <Dropdown
                className="rnd-option-select"
                value={value}
                label={option.label || option.name}
                onChange={onChange}
                options={[
                    ...(option.required && known ? [] : [{ value: "", label: "—" }]),
                    ...(value && !known ? [{ value, label: value, detail: "not allowed" }] : []),
                    ...choices.map((c) => ({ value: c.value, label: c.name, detail: c.name !== c.value ? c.value : undefined })),
                ]}
            />
        );
    }

    return (
        <>
            <input
                id={id}
                type="text"
                value={value}
                list={choices ? listId : undefined}
                placeholder={
                    option.isDate ? (option.dateFormat ?? "date") : option.multivalued ? `values separated by ${option.delimiter}` : undefined
                }
                onChange={(e) => onChange(e.target.value)}
                {...textProps}
            />
            {choices && (
                <datalist id={listId}>
                    {choices.map((c) => (
                        <option key={c.value} value={c.value}>
                            {c.name}
                        </option>
                    ))}
                </datalist>
            )}
        </>
    );
}
