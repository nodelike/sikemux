import { useState } from "react";
import { confirmDialog, notify } from "../../../plugin-api/host";
import { errorText } from "../../../plugin-api/format";
import { kubeApi, type KubeNode } from "../api";
import { age, cores, memory, type Tone } from "../format";
import { kubernetes } from "../resources";
import { Dot } from "./parts";

/** The colour a node reads in: not ready red, under pressure or cordoned amber. */
export function nodeTone(node: Pick<KubeNode, "status" | "pressure" | "cordoned">): Tone {
    if (node.status !== "Ready") return "danger";
    if (node.pressure.length > 0) return "warn";
    if (node.cordoned) return "idle";
    return "live";
}

/** A node's status, saying when it takes no new pods. */
export function statusLine(node: Pick<KubeNode, "status" | "cordoned">): string {
    if (!node.cordoned) return node.status;
    return node.status === "Ready" ? "Cordoned" : `${node.status}, cordoned`;
}

function Use({ used, of, write }: { used: number | null; of: number; write: (value: number) => string }) {
    if (used === null) return <span>{write(of)}</span>;
    const share = of === 0 ? 0 : Math.min(100, Math.round((used / of) * 100));
    return (
        <span className="k8s-use" title={`${write(used)} of ${write(of)}`}>
            <span>
                {write(used)} <span className="k8s-muted">/ {write(of)}</span>
            </span>
            <span className={`k8s-use-bar${share >= 85 ? " high" : ""}`} aria-hidden="true">
                <span style={{ width: `${share}%` }} />
            </span>
        </span>
    );
}

export function Nodes({ nodes, context }: { nodes: KubeNode[]; context: string }) {
    const [busy, setBusy] = useState<string | null>(null);
    const measured = nodes.some((node) => node.cpuUsedMillis !== null);

    const cordon = async (node: KubeNode) => {
        const cordoning = !node.cordoned;
        const yes = await confirmDialog({
            title: cordoning ? `Cordon ${node.name}?` : `Uncordon ${node.name}?`,
            body: cordoning
                ? "Pods running on it keep running, but no new pod is placed there until it is uncordoned."
                : "New pods can be placed on it again.",
            confirmLabel: cordoning ? "Cordon" : "Uncordon",
            commands: [{ label: "Runs", command: `kubectl --context ${context} ${cordoning ? "cordon" : "uncordon"} ${node.name}` }],
        });
        if (!yes) return;
        setBusy(node.name);
        try {
            const done = await kubeApi.cordon(context, node.name, cordoning);
            notify("success", done.message);
            kubernetes.refresh();
        } catch (failure) {
            notify("error", errorText(failure));
        } finally {
            setBusy(null);
        }
    };

    if (nodes.length === 0) return <p className="k8s-note">This cluster reports no nodes.</p>;
    return (
        <section className="k8s-section" aria-label="Nodes">
            {measured ? null : <p className="k8s-muted">CPU and memory use show once the cluster runs metrics-server.</p>}
            <table className="k8s-table" aria-label="Nodes">
                <thead>
                    <tr>
                        <th aria-label="Status" />
                        <th>Name</th>
                        <th>Status</th>
                        <th>Roles</th>
                        <th>CPU</th>
                        <th>Memory</th>
                        <th>Version</th>
                        <th>Age</th>
                        <th aria-label="Actions" />
                    </tr>
                </thead>
                <tbody>
                    {nodes.map((node) => (
                        <tr key={node.name} className="k8s-row" aria-label={node.name}>
                            <td className="k8s-cell-dot">
                                <Dot tone={nodeTone(node)} label={statusLine(node)} />
                            </td>
                            <td>
                                <span className="k8s-name">{node.name}</span>
                                <span className="k8s-note-inline k8s-mono" title={`${node.os}, ${node.runtime}`}>
                                    {[node.internalIp, node.arch].filter(Boolean).join(" · ")}
                                </span>
                                {node.reason ? <span className="k8s-note-inline failed">{node.reason}</span> : null}
                                {node.pressure.length > 0 || node.taints.length > 0 ? (
                                    <span className="k8s-chips">
                                        {node.pressure.map((pressure) => (
                                            <span key={pressure} className="k8s-chip warn">
                                                {pressure}
                                            </span>
                                        ))}
                                        {node.taints.map((taint) => (
                                            <span key={taint} className="k8s-chip k8s-mono" title="Taint">
                                                {taint}
                                            </span>
                                        ))}
                                    </span>
                                ) : null}
                            </td>
                            <td className={`k8s-status ${nodeTone(node)}`}>{statusLine(node)}</td>
                            <td className="k8s-muted">{node.roles.join(", ") || "—"}</td>
                            <td>
                                <Use used={node.cpuUsedMillis} of={node.cpuMillis} write={cores} />
                            </td>
                            <td>
                                <Use used={node.memoryUsedBytes} of={node.memoryBytes} write={memory} />
                            </td>
                            <td className="k8s-mono k8s-muted k8s-nowrap">{node.version}</td>
                            <td className="k8s-muted">{age(node.created)}</td>
                            <td className="k8s-cell-actions">
                                <span className="k8s-row-actions">
                                    <button
                                        type="button"
                                        className="k8s-button small"
                                        disabled={busy !== null}
                                        aria-label={`${node.cordoned ? "Uncordon" : "Cordon"} ${node.name}`}
                                        onClick={() => void cordon(node)}>
                                        {busy === node.name ? "…" : node.cordoned ? "Uncordon" : "Cordon"}
                                    </button>
                                </span>
                            </td>
                        </tr>
                    ))}
                </tbody>
            </table>
        </section>
    );
}
