import { useRef, type ReactNode } from "react";
import { useResourceEnabled, type ResourceHandle } from "../../../plugin-api/resources";
import { Dropdown, EmptyState, ResizeHandle, SkeletonRows, StaleNotice } from "../../../plugin-api/ui";
import type { KubeStatus, Scope } from "../api";
import { usePoll } from "../hooks";
import { kubeConfigR, kubeEventsR, kubeNamespacesR, kubeNetworkR, kubePodsR, kubeStatusR, kubeWorkloadsR, kubernetes } from "../resources";
import { ALL_NAMESPACES, updateKubeView, useKubeView, type KubeSection } from "../state";
import { KubernetesMark } from "./KubernetesMark";
import { Config } from "./Config";
import { Events } from "./Events";
import { Network } from "./Network";
import { Pods } from "./Pods";
import { PodView } from "./PodView";
import { Workloads } from "./Workloads";
import "../kubernetes.css";

const SIDE_MIN = 180;
const SIDE_MAX = 380;
const LIST_POLL_MS = 5_000;
const EVENTS_POLL_MS = 10_000;

export function KubernetesPane({ paneId, active }: { paneId: string; active: boolean }) {
    const status = useResourceEnabled(active, kubeStatusR);
    if (status.data === undefined && status.status !== "error") return <SkeletonRows rows={5} label="Reading your kubeconfig" />;
    if (!status.data?.ready) return <NotReady status={status.data} error={status.error} onRetry={() => void status.refresh()} />;
    return <Cluster paneId={paneId} active={active} status={status.data} />;
}

function NotReady({ status, error, onRetry }: { status: KubeStatus | undefined; error: string | undefined; onRetry: () => void }) {
    const missing = status?.category === "cli-missing";
    return (
        <div className="k8s-pane k8s-pane-empty">
            <EmptyState
                icon={<KubernetesMark size={28} />}
                title={missing ? "kubectl isn't installed" : "No clusters yet"}
                message={status?.message ?? error ?? "Sikemux couldn't read your kubeconfig."}
                action={{ label: "Try again", onClick: onRetry }}
            />
        </div>
    );
}

const SECTIONS: { value: KubeSection; label: string }[] = [
    { value: "workloads", label: "Workloads" },
    { value: "pods", label: "Pods" },
    { value: "network", label: "Services" },
    { value: "config", label: "Config" },
    { value: "events", label: "Events" },
];

function Cluster({ paneId, active, status }: { paneId: string; active: boolean; status: KubeStatus }) {
    const view = useKubeView(paneId);
    const sideRef = useRef<HTMLDivElement>(null);
    const context = view.context ?? status.current ?? status.contexts[0]?.name ?? "";
    const chosen = status.contexts.find((each) => each.name === context);
    const all = view.namespace === ALL_NAMESPACES;
    const namespace = all ? undefined : (view.namespace ?? chosen?.namespace ?? "default");
    const scope: Scope = { context, namespace };
    const namespaces = useResourceEnabled(active, kubeNamespacesR, context);
    const workloads = useResourceEnabled(active && view.section === "workloads", kubeWorkloadsR, scope, all);
    usePoll(active && view.pod === null && view.section === "workloads", LIST_POLL_MS, workloads.refresh);
    const listing = active && view.pod === null;
    const pods = useResourceEnabled(active && view.section === "pods", kubePodsR, scope, all, view.selector);
    usePoll(listing && view.section === "pods", LIST_POLL_MS, pods.refresh);
    const network = useResourceEnabled(listing && view.section === "network", kubeNetworkR, scope, all);
    const config = useResourceEnabled(listing && view.section === "config", kubeConfigR, scope, all);
    const events = useResourceEnabled(active, kubeEventsR, scope, all);
    usePoll(listing && view.section === "events", EVENTS_POLL_MS, events.refresh);
    const warnings = events.data?.filter((event) => event.kind === "Warning").length ?? 0;
    const trouble = workloads.data?.filter((each) => each.health !== "ok").length ?? 0;
    const counts: Partial<Record<KubeSection, string>> = {
        workloads: workloads.data ? (trouble > 0 ? `${trouble} !` : String(workloads.data.length)) : "",
        pods: pods.data ? String(pods.data.length) : "",
        network: network.data ? String(network.data.services.length) : "",
        events: warnings > 0 ? `${warnings} !` : "",
    };

    return (
        <div className="k8s-pane">
            <aside className="k8s-side" ref={sideRef} style={view.sideWidth ? { width: view.sideWidth } : undefined}>
                <Dropdown
                    className="k8s-pick"
                    label="Cluster"
                    value={context}
                    icon={
                        <span className="k8s-pick-mark">
                            <KubernetesMark size={12} />
                        </span>
                    }
                    options={status.contexts.map((each) => ({ value: each.name, label: each.name, detail: each.server ?? each.cluster }))}
                    onChange={(next) => updateKubeView(paneId, { context: next, namespace: null, pod: null, selector: null })}
                />
                <Dropdown
                    className="k8s-pick"
                    label="Namespace"
                    value={all ? ALL_NAMESPACES : (namespace ?? "")}
                    options={[
                        { value: ALL_NAMESPACES, label: "All namespaces" },
                        ...(namespaces.data ?? [{ name: namespace ?? "default", phase: "Active" }]).map((each) => ({
                            value: each.name,
                            label: each.name,
                            detail: each.phase === "Active" ? undefined : each.phase,
                        })),
                    ]}
                    onChange={(next) => updateKubeView(paneId, { namespace: next, pod: null, selector: null })}
                />
                <input
                    className="k8s-find"
                    value={view.find}
                    onChange={(event) => updateKubeView(paneId, { find: event.target.value })}
                    onKeyDown={(event) => {
                        if (event.key === "Escape") updateKubeView(paneId, { find: "" });
                    }}
                    placeholder="Find…"
                    aria-label="Find"
                    spellCheck={false}
                />
                <nav className="k8s-nav" aria-label="Kubernetes">
                    {SECTIONS.map((section) => (
                        <button
                            key={section.value}
                            type="button"
                            className={`k8s-nav-item${view.section === section.value ? " active" : ""}`}
                            aria-current={view.section === section.value ? "page" : undefined}
                            onClick={() => updateKubeView(paneId, { section: section.value, pod: null, selector: null })}>
                            <span>{section.label}</span>
                            <span className={`k8s-count${counts[section.value]?.endsWith("!") ? " trouble" : ""}`}>
                                {counts[section.value]?.replace(" !", "")}
                            </span>
                        </button>
                    ))}
                </nav>
                <div className="k8s-side-foot" title={chosen?.server ?? undefined}>
                    <span className="k8s-side-cluster">{chosen?.cluster ?? context}</span>
                    {chosen?.server ? <span className="k8s-muted k8s-mono">{chosen.server.replace(/^https?:\/\//, "")}</span> : null}
                </div>
            </aside>
            <ResizeHandle
                targetRef={sideRef}
                axis="x"
                grows={1}
                min={SIDE_MIN}
                max={() => SIDE_MAX}
                size={view.sideWidth}
                label="Resize the Kubernetes sidebar"
                className="k8s-resize"
                onResize={(size) => {
                    if (size === null && sideRef.current) sideRef.current.style.width = "";
                    updateKubeView(paneId, { sideWidth: size });
                }}
            />
            <main className="k8s-main">
                <header className="k8s-topbar">
                    <span className="k8s-topbar-title">{SECTIONS.find((each) => each.value === view.section)?.label}</span>
                    <span className="k8s-muted">{all ? "All namespaces" : namespace}</span>
                    <button type="button" className="k8s-button" onClick={() => kubernetes.refresh()}>
                        Refresh
                    </button>
                </header>
                <div className="k8s-page">
                    {view.pod ? (
                        <PodView
                            key={`${view.pod.namespace}/${view.pod.name}`}
                            scope={{ context, namespace: view.pod.namespace }}
                            name={view.pod.name}
                            active={active}
                            onBack={() => updateKubeView(paneId, { pod: null })}
                        />
                    ) : view.section === "pods" ? (
                        <Loaded handle={pods} label="pods">
                            {(data) => (
                                <Pods
                                    pods={data}
                                    find={view.find}
                                    selector={view.selector}
                                    showNamespace={all}
                                    onClearSelector={() => updateKubeView(paneId, { selector: null })}
                                    onOpen={(pod) => updateKubeView(paneId, { pod: { namespace: pod.namespace, name: pod.name } })}
                                />
                            )}
                        </Loaded>
                    ) : view.section === "network" ? (
                        <Loaded handle={network} label="services">
                            {(data) => (
                                <Network
                                    services={data.services}
                                    ingresses={data.ingresses}
                                    find={view.find}
                                    scope={scope}
                                    showNamespace={all}
                                    onPods={(service) =>
                                        updateKubeView(paneId, {
                                            section: "pods",
                                            selector: service.selector,
                                            namespace: all ? service.namespace : view.namespace,
                                        })
                                    }
                                />
                            )}
                        </Loaded>
                    ) : view.section === "config" ? (
                        <Loaded handle={config} label="config">
                            {(data) => <Config configMaps={data.configMaps} secrets={data.secrets} find={view.find} showNamespace={all} />}
                        </Loaded>
                    ) : view.section === "events" ? (
                        <Loaded handle={events} label="events">
                            {(data) => (
                                <Events
                                    events={data}
                                    find={view.find}
                                    onOpenPod={(event) =>
                                        updateKubeView(paneId, {
                                            pod: { namespace: event.namespace ?? namespace ?? "default", name: event.object.slice(4) },
                                        })
                                    }
                                />
                            )}
                        </Loaded>
                    ) : (
                        <Loaded handle={workloads} label="workloads">
                            {(data) => (
                                <Workloads
                                    workloads={data}
                                    find={view.find}
                                    scope={scope}
                                    showNamespace={all}
                                    onOpen={(workload) =>
                                        updateKubeView(paneId, {
                                            section: "pods",
                                            selector: workload.selector,
                                            namespace: all ? workload.namespace : view.namespace,
                                        })
                                    }
                                />
                            )}
                        </Loaded>
                    )}
                </div>
            </main>
        </div>
    );
}

function Loaded<T>({ handle, label, children }: { handle: ResourceHandle<T>; label: string; children: (data: T) => ReactNode }) {
    if (handle.status === "error" && handle.data === undefined) return <p className="k8s-note k8s-error">{handle.error}</p>;
    if (handle.data === undefined) return <SkeletonRows rows={6} label={`Loading ${label}`} />;
    return (
        <>
            <StaleNotice handle={handle} />
            {children(handle.data)}
        </>
    );
}
