import { useId, useState } from "react";
import { IconChevron, Switch } from "../../../plugin-api/ui";

export interface AdvancedRun {
    debug: boolean;
    nodeFilter: string;
    runAt: string;
    asUser: string;
}

export function RundeckAdvanced({
    value,
    onChange,
    defaultFilter,
}: {
    value: AdvancedRun;
    onChange: (next: AdvancedRun) => void;
    defaultFilter: string | null;
}) {
    const [open, setOpen] = useState(false);
    const id = useId();
    const changed = value.debug || value.runAt || value.asUser || value.nodeFilter !== (defaultFilter ?? "");
    const set = (patch: Partial<AdvancedRun>) => onChange({ ...value, ...patch });

    return (
        <div className="rnd-advanced">
            <button type="button" className="rnd-advanced-toggle" aria-expanded={open} aria-controls={id} onClick={() => setOpen((v) => !v)}>
                <IconChevron size={9} className={`rnd-tree-chev-ic${open ? " open" : ""}`} />
                advanced
                {changed && !open && <span className="rnd-tag">changed</span>}
            </button>
            {open && (
                <div className="rnd-advanced-body" id={id}>
                    <label className="rnd-toggle">
                        <Switch checked={value.debug} onChange={(debug) => set({ debug })} label="Debug log level" />
                        <span>debug output</span>
                    </label>
                    <label className="rnd-field">
                        <span>node filter</span>
                        <input
                            type="text"
                            value={value.nodeFilter}
                            placeholder={defaultFilter ?? "job's own nodes"}
                            onChange={(e) => set({ nodeFilter: e.target.value })}
                            spellCheck={false}
                            autoCapitalize="off"
                            autoCorrect="off"
                        />
                    </label>
                    <label className="rnd-field">
                        <span>run later</span>
                        <input type="datetime-local" value={value.runAt} onChange={(e) => set({ runAt: e.target.value })} />
                    </label>
                    <label className="rnd-field">
                        <span>run as user</span>
                        <input
                            type="text"
                            value={value.asUser}
                            placeholder="you"
                            onChange={(e) => set({ asUser: e.target.value })}
                            spellCheck={false}
                            autoCapitalize="off"
                            autoCorrect="off"
                        />
                    </label>
                </div>
            )}
        </div>
    );
}
