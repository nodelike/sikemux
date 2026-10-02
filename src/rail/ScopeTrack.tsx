import type { AgentRailScope } from "../state/types";

/** This project or every open project, as one track the choice slides along. */
export function ScopeTrack({
    scope,
    waitingElsewhere,
    onChange,
}: {
    scope: AgentRailScope;
    waitingElsewhere: number;
    onChange: (scope: AgentRailScope) => void;
}) {
    return (
        <div className="scope-track-row">
            <div className={`scope-track${scope === "all" ? " is-all" : ""}`} role="tablist" aria-label="Which projects">
                <span className="scope-thumb" aria-hidden="true" />
                <button type="button" role="tab" aria-selected={scope === "project"} onClick={() => onChange("project")}>
                    This project
                </button>
                <button
                    type="button"
                    role="tab"
                    aria-selected={scope === "all"}
                    aria-label={waitingElsewhere > 0 ? `All projects, ${waitingElsewhere} waiting` : "All projects"}
                    onClick={() => onChange("all")}>
                    All projects
                    {waitingElsewhere > 0 && <span className="scope-count">{waitingElsewhere}</span>}
                </button>
            </div>
        </div>
    );
}
