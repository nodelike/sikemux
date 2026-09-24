import { Dropdown, IconCheck, type DropdownOption } from "../../../plugin-api/ui";

export type BrunoOption = DropdownOption;

/** Bruno's alias for the app-wide dropdown. */
export const BrunoSelect = Dropdown;

/** Sharp custom checkbox. */
export function BrunoCheck({ checked, onChange, title }: { checked: boolean; onChange: (v: boolean) => void; title?: string }) {
    return (
        <button
            type="button"
            role="checkbox"
            aria-checked={checked}
            className={`bruno-check${checked ? " on" : ""}`}
            title={title}
            onClick={() => onChange(!checked)}>
            {checked && <IconCheck size={10} />}
        </button>
    );
}
