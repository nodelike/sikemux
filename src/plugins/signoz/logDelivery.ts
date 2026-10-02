import { codeFence, type AgentDelivery } from "../../plugin-api/ui";
import type { LogLine } from "./api";

function describeLine(line: LogLine): string {
    const head = [line.timestamp, line.severity ?? "-", line.service ?? "-", line.traceId ? `trace=${line.traceId}` : null].filter(Boolean).join(" ");
    return `${head}\n${line.body}`;
}

export function logDelivery(lines: readonly LogLine[]): AgentDelivery {
    const heading = lines.length === 1 ? "A log line from SigNoz:" : `${lines.length} log lines from SigNoz:`;
    return { text: `${heading}\n\n${codeFence(lines.map(describeLine).join("\n\n"))}\n` };
}
