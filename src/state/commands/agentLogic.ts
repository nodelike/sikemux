import type { AgentType } from "../types";

export const SKIP_PERMISSION_FLAG: Partial<Record<AgentType, string>> = {
    claude: "--dangerously-skip-permissions",
    hermes: "--yolo",
    codex: "--dangerously-bypass-approvals-and-sandbox",
    omp: "--approval-mode yolo",
    grok: "--permission-mode bypassPermissions",
    muse: "--yolo",
};

export function agentSupportsSkipPermissions(type: AgentType): boolean {
    return SKIP_PERMISSION_FLAG[type] != null;
}
