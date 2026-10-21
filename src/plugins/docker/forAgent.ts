import type { ContainerDetail, LogLine } from "./api";
import { portLabel, shortImage } from "./format";

const LOG_LINES_FOR_AGENT = 120;

/** A container as an agent reads it: what it runs, how it is doing, and the end of its log. */
export function containerForAgent(container: ContainerDetail, lines: readonly LogLine[]): string {
    const { state } = container;
    const facts = [
        `Image: ${shortImage(container.image)}`,
        container.compose
            ? `Compose: service ${container.compose.service} of ${container.compose.project}${container.compose.workingDir ? ` in ${container.compose.workingDir}` : ""}`
            : null,
        state.running
            ? `State: ${state.status}`
            : `State: exited with code ${state.exitCode}${state.oomKilled ? ", killed for running out of memory" : ""}`,
        state.error ? `Error: ${state.error}` : null,
        state.health
            ? `Health: ${state.health.status}${state.health.failingStreak > 0 ? `, ${state.health.failingStreak} checks failed in a row` : ""}`
            : null,
        container.restartCount > 0 ? `Restarts: ${container.restartCount} (policy ${container.restartPolicy})` : null,
        container.ports.length > 0 ? `Ports: ${container.ports.map(portLabel).join(", ")}` : null,
        container.env.length > 0 ? `Env names: ${container.env.join(", ")}` : null,
    ].filter(Boolean);
    const tail = lines.slice(-LOG_LINES_FOR_AGENT).map((line) => (line.stream === "stderr" ? `[err] ${line.text}` : line.text));
    return [
        `Docker container ${container.name} (${container.id})`,
        facts.join("\n"),
        tail.length > 0 ? `End of its log:\n\n\`\`\`\n${tail.join("\n")}\n\`\`\`` : null,
        `To read more, use docker_logs or docker_container with id ${container.name}.`,
    ]
        .filter(Boolean)
        .join("\n\n");
}
