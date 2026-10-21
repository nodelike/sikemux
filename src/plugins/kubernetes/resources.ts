import { pluginResources } from "../../plugin-api/service";
import { kubeApi, type Scope } from "./api";
import { KUBERNETES_PLUGIN } from "./kinds";

export const kubernetes = pluginResources(KUBERNETES_PLUGIN);

export const kubeStatusR = kubernetes.resource("status", () => kubeApi.status(), 30_000);

export const kubeNamespacesR = kubernetes.resource("namespaces", (context: string) => kubeApi.namespaces({ context }), 60_000);

export const kubePodsR = kubernetes.resource(
    "pods",
    (scope: Scope, all: boolean, selector: string | null) => kubeApi.pods(scope, all, selector),
    5_000,
);

export const kubeWorkloadsR = kubernetes.resource("workloads", (scope: Scope, all: boolean) => kubeApi.workloads(scope, all), 5_000);

export const kubeEventsR = kubernetes.resource("events", (scope: Scope, all: boolean) => kubeApi.events(scope, all), 10_000);

export const kubePodR = kubernetes.resource("pod", (scope: Scope, name: string) => kubeApi.pod(scope, name), 5_000);

export const kubeNetworkR = kubernetes.resource("network", (scope: Scope, all: boolean) => kubeApi.network(scope, all), 15_000);

export const kubeConfigR = kubernetes.resource("config", (scope: Scope, all: boolean) => kubeApi.config(scope, all), 30_000);
