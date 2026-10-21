import { useState } from "react";
import { copyWithNotice } from "../../../plugin-api/host";
import { Seg } from "../../../plugin-api/ui";
import type { Inventory, Resource } from "../api";
import { region } from "../format";
import { Dot } from "./parts";

/** The colour a resource's own word for its state reads in. */
export function stateTone(state: string | null): "live" | "warn" | "danger" | "idle" {
    if (!state) return "idle";
    const word = state.toLowerCase();
    if (word === "running" || word === "succeeded" || word === "available" || word === "ready" || word === "online") return "live";
    if (word === "failed" || word === "error" || word === "unavailable") return "danger";
    if (word === "stopped" || word === "deallocated" || word === "disabled") return "idle";
    return "warn";
}

export function Resources({ inventory, showGroup, onOpenGroup }: { inventory: Inventory; showGroup: boolean; onOpenGroup: (group: string) => void }) {
    const [kind, setKind] = useState<string>("all");
    const shown = kind === "all" ? inventory.resources : inventory.resources.filter((resource) => resource.kind === kind);
    const options = [
        { value: "all", label: "All", count: inventory.resources.length },
        ...inventory.kinds.slice(0, 7).map((each) => ({ value: each.kind, label: each.label, count: each.count })),
    ];

    return (
        <section className="az-section" aria-label="Resources">
            <div className="az-toolbar">
                <Seg label="Type" value={kind} onChange={setKind} options={options} />
            </div>
            {inventory.truncated ? <p className="az-note">Azure sent only the first 1,000 resources. Narrow it by group or words.</p> : null}
            {shown.length === 0 ? (
                <p className="az-note">{inventory.resources.length === 0 ? "Nothing here yet." : "Nothing of that type."}</p>
            ) : (
                <table className="az-table" aria-label="Resources">
                    <thead>
                        <tr>
                            <th aria-label="State" />
                            <th>Name</th>
                            <th>Type</th>
                            {showGroup ? <th>Resource group</th> : null}
                            <th>Region</th>
                            <th>Size</th>
                        </tr>
                    </thead>
                    <tbody>
                        {shown.map((resource: Resource) => (
                            <tr key={resource.id} aria-label={resource.name}>
                                <td className="az-cell-dot">
                                    <Dot tone={stateTone(resource.state)} label={resource.state ?? "unknown"} />
                                </td>
                                <td>
                                    <button
                                        type="button"
                                        className="az-name az-link"
                                        title="Copy the resource id"
                                        onClick={() => copyWithNotice(resource.id, "The resource id")}>
                                        {resource.name}
                                    </button>
                                </td>
                                <td className="az-muted">{resource.label}</td>
                                {showGroup ? (
                                    <td>
                                        <button type="button" className="az-link" onClick={() => onOpenGroup(resource.group)}>
                                            {resource.group}
                                        </button>
                                    </td>
                                ) : null}
                                <td className="az-muted">{region(resource.location)}</td>
                                <td className="az-muted az-mono">{resource.sku ?? "—"}</td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            )}
        </section>
    );
}
