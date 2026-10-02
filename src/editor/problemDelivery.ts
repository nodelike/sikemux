import { codeFence } from "../agents/agentTargets";
import type { AgentDelivery } from "../agents/agentInbox";
import { basename, relativePath } from "../lib/paths";
import type { DiagnosticProblem } from "../workbench/diagnosticsController";

export function problemLocation(project: string, problem: Pick<DiagnosticProblem, "path" | "range">): string {
    const relative = relativePath(problem.path, project);
    return `${relative || basename(problem.path)}:${problem.range.start.line + 1}:${problem.range.start.character + 1}`;
}

export function problemDelivery(project: string, problem: DiagnosticProblem): AgentDelivery {
    const kind = problem.severity ?? "problem";
    const source = [problem.source, problem.code].filter(Boolean).join(" ");
    const heading = `${kind[0].toUpperCase()}${kind.slice(1)} at ${problemLocation(project, problem)}${source ? ` from ${source}` : ""}:`;
    const message = problem.message.includes("\n") ? codeFence(problem.message) : problem.message;
    return { text: `${heading}\n\n${message}\n`, paths: [problem.path] };
}
