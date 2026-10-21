import type { PodDetail, Scope } from "./api";

/** What an agent is told about a pod in trouble, so it can start from what the pane already shows. */
export function podBrief(pod: PodDetail, scope: Scope): string {
    const where = [scope.context ? `cluster ${scope.context}` : null, `namespace ${pod.namespace}`].filter(Boolean).join(", ");
    const lines = [`Pod ${pod.name} (${where}) is ${pod.status}, ${pod.ready}/${pod.containers} ready, ${pod.restarts} restarts.`];
    if (pod.owner) lines.push(`It belongs to ${pod.owner.kind} ${pod.owner.name}.`);
    for (const container of pod.containerStates) {
        const state = [container.state, container.reason, container.exitCode === null ? null : `exit ${container.exitCode}`]
            .filter(Boolean)
            .join(", ");
        lines.push(`- ${container.init ? "init container" : "container"} ${container.name} (${container.image}): ${state}`);
        if (container.message) lines.push(`  ${container.message}`);
        if (container.last)
            lines.push(
                `  last run ended: ${container.last.reason ?? "exited"}${container.last.exitCode === null ? "" : ` (exit ${container.last.exitCode})`}`,
            );
    }
    const warnings = pod.events.filter((event) => event.kind === "Warning").slice(0, 8);
    if (warnings.length > 0) {
        lines.push("", "Recent warnings:");
        for (const event of warnings) lines.push(`- ${event.reason}${event.count > 1 ? ` ×${event.count}` : ""}: ${event.message}`);
    }
    lines.push(
        "",
        "Find out why, using the k8s_ tools: k8s_logs with previous for the run before the last restart, k8s_pod and k8s_events for more. Say what is wrong and what to change before changing anything.",
    );
    return lines.join("\n");
}
