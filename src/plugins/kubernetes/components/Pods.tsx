import { useState } from "react";
import { Seg } from "../../../plugin-api/ui";
import type { Pod } from "../api";
import { age, podTone } from "../format";
import { Dot } from "./parts";

type PodFilter = "all" | "trouble" | "running" | "done";

export function keeps(pod: Pod, find: string, filter: PodFilter): boolean {
    const tone = podTone(pod);
    if (filter === "trouble" && tone !== "danger" && tone !== "warn") return false;
    if (filter === "running" && pod.status !== "Running") return false;
    if (filter === "done" && tone !== "idle") return false;
    const words = find.trim().toLowerCase();
    return !words || pod.name.includes(words) || (pod.owner?.name.includes(words) ?? false) || (pod.node?.includes(words) ?? false);
}

export function Pods({
    pods,
    find,
    selector,
    showNamespace,
    onClearSelector,
    onOpen,
}: {
    pods: Pod[];
    find: string;
    /** The workload the list is narrowed to, by its label selector. */
    selector: string | null;
    showNamespace: boolean;
    onClearSelector: () => void;
    onOpen: (pod: Pod) => void;
}) {
    const [filter, setFilter] = useState<PodFilter>("all");
    const found = pods.filter((pod) => keeps(pod, find, "all"));
    const count = (each: PodFilter) => found.filter((pod) => keeps(pod, "", each)).length;
    const shown = found.filter((pod) => keeps(pod, "", filter));
    return (
        <section className="k8s-section" aria-label="Pods">
            <div className="k8s-toolbar">
                <Seg
                    label="Show"
                    value={filter}
                    onChange={setFilter}
                    options={[
                        { value: "all", label: "All", count: count("all") },
                        { value: "trouble", label: "Needs attention", count: count("trouble") },
                        { value: "running", label: "Running", count: count("running") },
                        { value: "done", label: "Completed", count: count("done") },
                    ]}
                />
                {selector ? (
                    <button type="button" className="k8s-chip k8s-chip-button" title="Show every pod" onClick={onClearSelector}>
                        {selector} ✕
                    </button>
                ) : null}
            </div>
            {shown.length === 0 ? (
                <p className="k8s-note">{pods.length === 0 ? "No pods here." : "No pods match."}</p>
            ) : (
                <table className="k8s-table" aria-label="Pods">
                    <thead>
                        <tr>
                            <th aria-label="Status" />
                            <th>Name</th>
                            <th>Status</th>
                            <th>Ready</th>
                            <th className="k8s-num">Restarts</th>
                            <th>Age</th>
                            <th>Node</th>
                        </tr>
                    </thead>
                    <tbody>
                        {shown.map((pod) => (
                            <tr
                                key={`${pod.namespace}/${pod.name}`}
                                className="k8s-row"
                                tabIndex={0}
                                aria-label={pod.name}
                                onClick={() => onOpen(pod)}
                                onKeyDown={(event) => {
                                    if (event.key === "Enter") onOpen(pod);
                                }}>
                                <td className="k8s-cell-dot">
                                    <Dot tone={podTone(pod)} label={pod.status} />
                                </td>
                                <td>
                                    <span className="k8s-name">{pod.name}</span>
                                    {showNamespace ? <span className="k8s-chip">{pod.namespace}</span> : null}
                                    {pod.owner ? (
                                        <span className="k8s-note-inline">
                                            {pod.owner.kind} {pod.owner.name}
                                        </span>
                                    ) : null}
                                </td>
                                <td className={`k8s-status ${podTone(pod)}`}>{pod.status}</td>
                                <td className="k8s-num-left">
                                    {pod.ready}/{pod.containers}
                                </td>
                                <td className={`k8s-num${pod.restarts > 0 ? " k8s-restarts" : ""}`}>{pod.restarts}</td>
                                <td className="k8s-muted">{age(pod.created)}</td>
                                <td className="k8s-muted k8s-mono">{pod.node ?? "—"}</td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            )}
        </section>
    );
}
