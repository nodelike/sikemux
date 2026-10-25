import type { TabRailScope } from "../state/types";
import { IconFolder, IconStack } from "@shared/ui/Icons";

/** This project, by name, or every open project, as one track the choice slides along. */
export function ScopeTabs({
    scope,
    name,
    waitingElsewhere = 0,
    onChange,
}: {
    scope: TabRailScope;
    name: string;
    waitingElsewhere?: number;
    onChange: (scope: TabRailScope) => void;
}) {
    return (
        <div className={`scope-track${scope === "all" ? " is-all" : ""}`} role="tablist" aria-label="Which projects">
            <span className="scope-thumb" aria-hidden="true" />
            <button type="button" role="tab" aria-selected={scope === "project"} aria-label={name} onClick={() => onChange("project")}>
                <IconFolder size={13} />
                <span className="scope-name">{name}</span>
            </button>
            <button
                type="button"
                role="tab"
                aria-selected={scope === "all"}
                aria-label={waitingElsewhere > 0 ? `All projects, ${waitingElsewhere} waiting` : "All projects"}
                onClick={() => onChange("all")}>
                <IconStack size={13} />
                All projects
                {waitingElsewhere > 0 && <span className="scope-count">{waitingElsewhere}</span>}
            </button>
        </div>
    );
}
