import type { GkeCluster, Pod, PodLogLine, RunServiceDetail, VmDetail } from "./api";

const LINES_FOR_AGENT = 120;

/** A Cloud Run service as an agent reads it: where it stands, what it runs, and how to read more. */
export function serviceForAgent(project: string, service: RunServiceDetail): string {
    const traffic = service.traffic
        .filter((entry) => entry.percent > 0)
        .map((entry) => `${entry.percent}% ${entry.latest ? "latest" : entry.revision}`);
    return [
        `Cloud Run service ${service.name} in ${project} (${service.location}): ${service.state}`,
        [
            service.message && `Status: ${service.message}`,
            service.url && `URL: ${service.url}`,
            service.image && `Image: ${service.image}`,
            `Latest ready revision: ${service.latestRevision}`,
            traffic.length > 0 && `Traffic: ${traffic.join(", ")}`,
            service.env.length > 0 && `Environment variables: ${service.env.map((entry) => entry.name).join(", ")}`,
        ]
            .filter(Boolean)
            .join("\n"),
        `To read its logs, use gcp_logs with resource runService and name ${service.name}; to roll back, gcp_act run_traffic with a revision.`,
    ].join("\n\n");
}

/** A pod's logs as an agent reads them: why it is failing and the end of what it printed. */
export function podForAgent(project: string, cluster: GkeCluster, pod: Pod, lines: PodLogLine[], previous: boolean): string {
    return [
        `Pod ${pod.namespace}/${pod.name} in GKE cluster ${cluster.name} (${cluster.location}, project ${project})`,
        [
            `Phase: ${pod.phase}`,
            pod.reason && `Reason: ${pod.reason}`,
            `Ready: ${pod.ready}`,
            `Restarts: ${pod.restarts}`,
            pod.node && `Node: ${pod.node}`,
        ]
            .filter(Boolean)
            .join("\n"),
        `${previous ? "Logs of the run before its last restart" : "End of its logs"}:\n\n\`\`\`\n${lines
            .slice(-LINES_FOR_AGENT)
            .map((line) => line.text)
            .join("\n")}\n\`\`\``,
        `To read more, use gcp_kube with cluster ${cluster.name}, location ${cluster.location}, what logs, namespace ${pod.namespace} and pod ${pod.name}.`,
    ].join("\n\n");
}

/** A VM's serial output as an agent reads it: the machine, then the end of its console. */
export function serialForAgent(project: string, vm: VmDetail, contents: string): string {
    const tail = contents.split("\n").slice(-LINES_FOR_AGENT).join("\n");
    return [
        `VM ${vm.name} in ${project} (${vm.zone}, ${vm.machineType}): ${vm.status}`,
        `End of its serial console:\n\n\`\`\`\n${tail}\n\`\`\``,
        `To read more, use gcp_get with kind serial, name ${vm.name} and location ${vm.zone}.`,
    ].join("\n\n");
}
