import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import { KUBERNETES_CLUSTER } from "./kinds";

export type KubeSection = "workloads" | "pods" | "network" | "config" | "events" | "nodes" | "helm";

/** Every namespace at once, as kubectl's --all-namespaces. */
export const ALL_NAMESPACES = "*";

export interface KubeView {
    /** The cluster shown, by context name; none for kubectl's current one. */
    context: string | null;
    /** The namespace shown; none for the context's own, ALL_NAMESPACES for every one. */
    namespace: string | null;
    section: KubeSection;
    /** The pod open, by namespace and name. */
    pod: { namespace: string; name: string } | null;
    /** The workload open, by kind, namespace and name. */
    workload: { kind: string; namespace: string; name: string } | null;
    /** The Helm release open, by namespace and name. */
    release: { namespace: string; name: string } | null;
    /** Pods narrowed to a workload's, by its label selector. */
    selector: string | null;
    find: string;
    sideWidth: number | null;
}

const views = definePaneViews<KubeView>({
    context: null,
    namespace: null,
    section: "workloads",
    pod: null,
    workload: null,
    release: null,
    selector: null,
    find: "",
    sideWidth: null,
});

export const useKubeView = views.use;
export const updateKubeView = views.update;

export const openKubernetes = (): string | null => openSurface(KUBERNETES_CLUSTER);
