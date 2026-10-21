import { Seg } from "../../../plugin-api/ui";
import { useState } from "react";
import type { Scope, Workload } from "../api";
import { age, shortImage, workloadTone } from "../format";
import { Dot, ReadyBar } from "./parts";
import { WorkloadActions } from "./WorkloadActions";

type KindFilter = "all" | "trouble" | "Deployment" | "StatefulSet" | "DaemonSet" | "CronJob" | "Job";

export function matches(workload: Workload, find: string, filter: KindFilter): boolean {
    if (filter === "trouble" && workload.health === "ok") return false;
    if (filter !== "all" && filter !== "trouble" && workload.kind !== filter) return false;
    const words = find.trim().toLowerCase();
    return !words || workload.name.includes(words) || workload.images.some((image) => image.includes(words));
}

export function Workloads({
    workloads,
    find,
    scope,
    showNamespace,
    onOpen,
}: {
    workloads: Workload[];
    find: string;
    scope: Scope;
    showNamespace: boolean;
    onOpen: (workload: Workload) => void;
}) {
    const [filter, setFilter] = useState<KindFilter>("all");
    const found = workloads.filter((each) => matches(each, find, "all"));
    const count = (kind: KindFilter) => found.filter((each) => matches(each, "", kind)).length;
    const shown = found.filter((each) => matches(each, "", filter));
    const options = (["all", "trouble", "Deployment", "StatefulSet", "DaemonSet", "CronJob", "Job"] as const)
        .map((value) => ({ value, label: value === "all" ? "All" : value === "trouble" ? "Needs attention" : `${value}s`, count: count(value) }))
        .filter((option) => option.value === "all" || option.value === "trouble" || option.count > 0);
    return (
        <section className="k8s-section" aria-label="Workloads">
            <div className="k8s-toolbar">
                <Seg label="Show" options={options} value={filter} onChange={setFilter} />
            </div>
            {shown.length === 0 ? (
                <p className="k8s-note">{workloads.length === 0 ? "No workloads in this namespace." : "No workloads match."}</p>
            ) : (
                <table className="k8s-table" aria-label="Workloads">
                    <thead>
                        <tr>
                            <th aria-label="Health" />
                            <th>Name</th>
                            <th>Kind</th>
                            <th>Ready</th>
                            <th>Images</th>
                            <th>Age</th>
                            <th aria-label="Actions" />
                        </tr>
                    </thead>
                    <tbody>
                        {shown.map((workload) => (
                            <tr
                                key={`${workload.namespace}/${workload.kind}/${workload.name}`}
                                className="k8s-row"
                                tabIndex={0}
                                aria-label={`${workload.kind} ${workload.name}`}
                                onClick={() => onOpen(workload)}
                                onKeyDown={(event) => {
                                    if (event.key === "Enter") onOpen(workload);
                                }}>
                                <td className="k8s-cell-dot">
                                    <Dot tone={workloadTone(workload)} label={workload.note ?? workload.health} />
                                </td>
                                <td>
                                    <span className="k8s-name">{workload.name}</span>
                                    {showNamespace ? <span className="k8s-chip">{workload.namespace}</span> : null}
                                    {workload.note ? <span className={`k8s-note-inline ${workload.health}`}>{workload.note}</span> : null}
                                </td>
                                <td className="k8s-muted">
                                    {workload.kind}
                                    {workload.schedule ? <span className="k8s-mono"> · {workload.schedule}</span> : null}
                                    {workload.suspended ? <span className="k8s-chip">paused</span> : null}
                                </td>
                                <td>
                                    {workload.ready !== null && workload.desired !== null ? (
                                        <ReadyBar ready={workload.ready} of={workload.desired} />
                                    ) : (
                                        <span className="k8s-muted">—</span>
                                    )}
                                </td>
                                <td className="k8s-mono k8s-images" title={workload.images.join("\n")}>
                                    {workload.images.map(shortImage).join(", ")}
                                </td>
                                <td className="k8s-muted">{age(workload.created)}</td>
                                <td className="k8s-cell-actions">
                                    <WorkloadActions workload={workload} scope={{ context: scope.context, namespace: workload.namespace }} />
                                </td>
                            </tr>
                        ))}
                    </tbody>
                </table>
            )}
        </section>
    );
}
