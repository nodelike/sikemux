import type { JobOption } from "../api";

export type OptionValues = Record<string, string>;

/** Allowed values fetched from an option's remote URL, by option name; null when the fetch failed. */
export type RemoteValues = Record<string, string[] | null | undefined>;

const REF = /\$\{option\.([^.}]+)\.value\}/g;

export function optionDelimiter(option: JobOption): string {
    return option.delimiter || ",";
}

export function splitMulti(value: string, delimiter: string): string[] {
    return value
        .split(delimiter)
        .map((part) => part.trim())
        .filter(Boolean);
}

export function initialOptionValues(
    options: JobOption[],
    prefill: OptionValues | undefined,
    branchKey: string | null,
    branch: string | undefined,
): OptionValues {
    const values: OptionValues = {};
    for (const option of options) {
        if (option.secure || option.kind === "file") values[option.name] = "";
        else values[option.name] = prefill?.[option.name] ?? option.default ?? "";
    }
    if (branchKey && branch !== undefined && branch !== "") values[branchKey] = branch;
    return values;
}

/** The names a remote values URL depends on through `${option.NAME.value}`. */
export function referencedOptions(url: string): string[] {
    return [...url.matchAll(REF)].map((match) => match[1]);
}

export function substituteOptionRefs(url: string, values: OptionValues): string {
    return url.replace(REF, (_, name: string) => encodeURIComponent(values[name] ?? ""));
}

export function allowedValues(option: JobOption, remote: RemoteValues): string[] | null {
    if (option.values_url) return remote[option.name] ?? null;
    return option.values && option.values.length ? option.values : null;
}

export function validateOptions(options: JobOption[], values: OptionValues, remote: RemoteValues): Record<string, string> {
    const errors: Record<string, string> = {};
    for (const option of options) {
        const value = (values[option.name] ?? "").trim();
        if (option.kind === "file") {
            if (option.required) errors[option.name] = "file options must be run from Rundeck";
            continue;
        }
        if (!value) {
            if (option.required) errors[option.name] = "required";
            continue;
        }
        const allowed = option.enforced ? allowedValues(option, remote) : null;
        if (!allowed) continue;
        const picked = option.multivalued ? splitMulti(value, optionDelimiter(option)) : [value];
        const bad = picked.find((entry) => !allowed.includes(entry));
        if (bad !== undefined) errors[option.name] = `"${bad}" is not one of the allowed values`;
    }
    return errors;
}

/** What goes to Rundeck: every filled-in option except files, which only Rundeck's own form can upload. */
export function runOptionValues(options: JobOption[], values: OptionValues): OptionValues {
    const out: OptionValues = {};
    for (const option of options) {
        if (option.kind === "file") continue;
        const value = (values[option.name] ?? "").trim();
        if (value) out[option.name] = value;
    }
    return out;
}

/** Options from a past run that may be shown and reused: never the secure ones. */
export function reusableOptions(options: OptionValues | null | undefined, secureNames: Set<string>): OptionValues {
    const out: OptionValues = {};
    for (const [name, value] of Object.entries(options ?? {})) {
        if (!secureNames.has(name)) out[name] = value;
    }
    return out;
}
