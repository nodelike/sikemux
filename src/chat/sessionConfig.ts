import type { ConfigPicker } from "../api/generated/wire";

/** The name of the value a picker holds, or the value itself when its choices do not list it. */
export function currentLabel(picker: ConfigPicker | null | undefined): string | undefined {
    if (!picker) return undefined;
    return picker.choices.find((choice) => choice.value === picker.current)?.label ?? (picker.current || undefined);
}
