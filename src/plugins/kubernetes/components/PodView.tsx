import { useState } from "react";
import { confirmDialog, copyWithNotice, notify, runInTerminal } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { SendToAgentMenu, Seg, SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import { podBrief } from "../brief";
import { errorText } from "../../../plugin-api/format";
import { kubeApi, type Container, type KubeEvent, type PodDetail, type Scope } from "../api";
import { age, podTone, shortImage, type Tone } from "../format";
import { usePoll } from "../hooks";
import { kubePodR, kubernetes } from "../resources";
import { Dot } from "./parts";
import { PodLogs } from "./PodLogs";

const DETAIL_POLL_MS = 5_000;

type PodTab = "overview" | "logs";

export function kubectlScope(scope: Scope): string[] {
    return [...(scope.context ? ["--context", scope.context] : []), ...(scope.namespace ? ["--namespace", scope.namespace] : [])];
}

export function PodView({ scope, name, active, onBack }: { scope: Scope; name: string; active: boolean; onBack: () => void }) {
    const [tab, setTab] = useState<PodTab>("overview");
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const pod = useResourceEnabled(active, kubePodR, scope, name);
    usePoll(active && tab === "overview", DETAIL_POLL_MS, pod.refresh);
    const detail = pod.data;

    const shell = () => {
        const container = detail?.containerStates.find((each) => !each.init && each.state === "running")?.name;
        void runInTerminal(`Shell in ${name}`, [
            "kubectl",
            ...kubectlScope(scope),
            "exec",
            "--stdin",
            "--tty",
            name,
            ...(container ? ["--container", container] : []),
            "--",
            "sh",
            "-c",
            "command -v bash >/dev/null && exec bash || exec sh",
        ]);
    };

    const remove = async () => {
        const owned = detail?.owner !== null && detail?.owner !== undefined;
        const confirmed = await confirmDialog({
            title: `Delete ${name}?`,
            body: owned ? `Its ${detail?.owner?.kind} starts a new pod in its place.` : "Nothing owns this pod, so nothing will start it again.",
            confirmLabel: "Delete pod",
            destructive: !owned,
            commands: [{ label: "Runs", command: ["kubectl", ...kubectlScope(scope), "delete", "pod", name].join(" ") }],
        });
        if (!confirmed) return;
        try {
            const done = await kubeApi.act(scope, "deletePod", name);
            notify("success", done.message || `Deleted ${name}`);
            kubernetes.refresh();
            onBack();
        } catch (failure) {
            notify("error", errorText(failure));
        }
    };

    return (
        <section className="k8s-detail" aria-label={`Pod ${name}`}>
            <header className="k8s-detail-head">
                <button type="button" className="k8s-back" onClick={onBack} aria-label="Back to pods">
                    ‹ Pods
                </button>
                <div className="k8s-detail-title">
                    {detail ? <Dot tone={podTone(detail)} label={detail.status} /> : null}
                    <h2 className="k8s-name">{name}</h2>
                    {detail ? <span className={`k8s-status ${podTone(detail)}`}>{detail.status}</span> : null}
                </div>
                <div className="k8s-detail-actions">
                    <button
                        type="button"
                        className="k8s-button"
                        disabled={!detail}
                        onClick={(event) => {
                            const box = event.currentTarget.getBoundingClientRect();
                            setMenu({ x: box.right, y: box.bottom + 4 });
                        }}>
                        Ask an agent
                    </button>
                    <button type="button" className="k8s-button" onClick={() => copyWithNotice(name, "Pod name")}>
                        Copy name
                    </button>
                    <button type="button" className="k8s-button" disabled={detail?.status !== "Running"} onClick={shell}>
                        Shell
                    </button>
                    <button type="button" className="k8s-button danger" disabled={!detail} onClick={() => void remove()}>
                        Delete
                    </button>
                </div>
            </header>
            {menu && detail ? (
                <SendToAgentMenu
                    x={menu.x}
                    y={menu.y}
                    alignRight
                    delivery={() => ({ text: podBrief(detail, scope) })}
                    onClose={() => setMenu(null)}
                />
            ) : null}
            <Seg
                label="Pod view"
                value={tab}
                onChange={setTab}
                options={[
                    { value: "overview", label: "Overview" },
                    { value: "logs", label: "Logs" },
                ]}
            />
            {pod.status === "error" && !detail ? (
                <p className="k8s-note k8s-error">{pod.error}</p>
            ) : detail === null ? (
                <p className="k8s-note">This pod is gone. Its workload may have replaced it.</p>
            ) : !detail ? (
                <SkeletonRows rows={6} label="Reading the pod" />
            ) : tab === "logs" ? (
                <PodLogs
                    scope={scope}
                    pod={name}
                    containers={detail.containerStates.filter((each) => !each.init).map((each) => each.name)}
                    restarted={detail.restarts > 0}
                />
            ) : (
                <>
                    <StaleNotice handle={pod} />
                    <Overview pod={detail} />
                </>
            )}
        </section>
    );
}

function Overview({ pod }: { pod: PodDetail }) {
    return (
        <div className="k8s-overview">
            <dl className="k8s-facts">
                <Fact term="Node" value={pod.node} mono />
                <Fact term="Pod IP" value={pod.ip} mono />
                <Fact term="Owner" value={pod.owner ? `${pod.owner.kind} ${pod.owner.name}` : "None"} />
                <Fact term="Age" value={age(pod.created)} />
                <Fact term="Ready" value={`${pod.ready}/${pod.containers}`} />
                <Fact term="Restarts" value={String(pod.restarts)} />
            </dl>
            <h3 className="k8s-heading">Containers</h3>
            <div className="k8s-containers">
                {pod.containerStates.map((container) => (
                    <ContainerCard key={`${container.init ? "init" : "main"}-${container.name}`} container={container} />
                ))}
            </div>
            {pod.conditions.length > 0 ? (
                <>
                    <h3 className="k8s-heading">Conditions</h3>
                    <ul className="k8s-conditions">
                        {pod.conditions.map((condition) => (
                            <li key={condition.kind} className={condition.ok ? "ok" : "not"}>
                                <span aria-hidden="true">{condition.ok ? "✓" : "✕"}</span>
                                <span>{condition.kind}</span>
                                {condition.reason ? <span className="k8s-muted">{condition.reason}</span> : null}
                                {condition.message ? <span className="k8s-muted">{condition.message}</span> : null}
                            </li>
                        ))}
                    </ul>
                </>
            ) : null}
            <h3 className="k8s-heading">Events</h3>
            <EventList events={pod.events} empty="No events in the last hour." />
            {pod.volumes.length > 0 ? (
                <>
                    <h3 className="k8s-heading">Volumes</h3>
                    <ul className="k8s-pairs">
                        {pod.volumes.map((volume) => (
                            <li key={volume.name}>
                                <span className="k8s-mono">{volume.name}</span>
                                <span className="k8s-muted">
                                    {volume.kind}
                                    {volume.source ? ` · ${volume.source}` : ""}
                                </span>
                            </li>
                        ))}
                    </ul>
                </>
            ) : null}
            {pod.labels.length > 0 ? (
                <>
                    <h3 className="k8s-heading">Labels</h3>
                    <div className="k8s-labels">
                        {pod.labels.map(([key, value]) => (
                            <span key={key} className="k8s-chip k8s-mono">
                                {key}={value}
                            </span>
                        ))}
                    </div>
                </>
            ) : null}
        </div>
    );
}

function Fact({ term, value, mono }: { term: string; value: string | null; mono?: boolean }) {
    return (
        <div>
            <dt>{term}</dt>
            <dd className={mono ? "k8s-mono" : undefined}>{value ?? "—"}</dd>
        </div>
    );
}

export function stateLine(container: Container): string {
    if (container.state === "running") return container.since ? `Running for ${age(container.since)}` : "Running";
    if (container.state === "terminated") {
        const code = container.exitCode === null ? "" : ` with code ${container.exitCode}`;
        return `${container.reason ?? "Exited"}${code}`;
    }
    return container.reason ?? "Waiting";
}

export function containerTone(container: Container): Tone {
    if (container.state === "running") return container.ready ? "live" : "warn";
    if (container.state === "terminated") return container.exitCode === 0 ? "idle" : "danger";
    return /BackOff|Err/.test(container.reason ?? "") ? "danger" : "warn";
}

function ContainerCard({ container }: { container: Container }) {
    return (
        <article className="k8s-container" aria-label={`Container ${container.name}`}>
            <header>
                <Dot tone={containerTone(container)} />
                <span className="k8s-name">{container.name}</span>
                {container.init ? <span className="k8s-chip">init</span> : null}
                <span className="k8s-muted">{stateLine(container)}</span>
                {container.restarts > 0 ? <span className="k8s-restarts">{container.restarts} restarts</span> : null}
            </header>
            {container.message ? <p className="k8s-container-message">{container.message}</p> : null}
            {container.last ? (
                <p className="k8s-muted">
                    Last run ended: {container.last.reason ?? "Exited"}
                    {container.last.exitCode === null ? "" : ` (code ${container.last.exitCode})`}
                    {container.last.finished ? `, ${age(container.last.finished)} ago` : ""}
                </p>
            ) : null}
            <dl className="k8s-facts compact">
                <Fact term="Image" value={shortImage(container.image)} mono />
                {container.ports.length > 0 ? <Fact term="Ports" value={container.ports.join(", ")} mono /> : null}
                {container.requests.length > 0 ? (
                    <Fact term="Requests" value={container.requests.map(([key, value]) => `${key} ${value}`).join(", ")} />
                ) : null}
                {container.limits.length > 0 ? (
                    <Fact term="Limits" value={container.limits.map(([key, value]) => `${key} ${value}`).join(", ")} />
                ) : null}
                {container.probes.length > 0 ? <Fact term="Probes" value={container.probes.join(" · ")} /> : null}
            </dl>
            {container.env.length > 0 || container.envFrom.length > 0 ? (
                <details className="k8s-env">
                    <summary>Environment ({container.env.length + container.envFrom.length})</summary>
                    <ul className="k8s-pairs">
                        {container.envFrom.map((source) => (
                            <li key={`from-${source}`}>
                                <span className="k8s-muted">all of</span>
                                <span className="k8s-mono">{source}</span>
                            </li>
                        ))}
                        {container.env.map((variable) => (
                            <li key={variable.name}>
                                <span className="k8s-mono">{variable.name}</span>
                                <span className="k8s-muted">{variable.from}</span>
                            </li>
                        ))}
                    </ul>
                </details>
            ) : null}
        </article>
    );
}

export function EventList({
    events,
    empty,
    showObject,
    onOpenPod,
}: {
    events: KubeEvent[];
    empty: string;
    showObject?: boolean;
    onOpenPod?: (name: string) => void;
}) {
    if (events.length === 0) return <p className="k8s-note">{empty}</p>;
    return (
        <ul className="k8s-events" aria-label="Events">
            {events.map((event, index) => (
                <li key={`${event.object}-${event.reason}-${index}`} className={event.kind === "Warning" ? "warning" : undefined}>
                    <span className="k8s-event-reason">{event.reason}</span>
                    <span className="k8s-event-message">
                        {showObject && onOpenPod && event.object.startsWith("Pod/") ? (
                            <button type="button" className="k8s-link k8s-mono" onClick={() => onOpenPod(event.object.slice(4))}>
                                {event.object}
                            </button>
                        ) : showObject ? (
                            <span className="k8s-mono k8s-muted">{event.object}</span>
                        ) : null}{" "}
                        {event.message}
                    </span>
                    <span className="k8s-muted k8s-event-when">
                        {event.count > 1 ? `×${event.count} · ` : ""}
                        {age(event.last)}
                    </span>
                </li>
            ))}
        </ul>
    );
}
