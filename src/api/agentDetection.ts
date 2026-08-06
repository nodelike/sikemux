import { invoke } from "@tauri-apps/api/core";

export interface ManifestReport {
    manifests: Array<{ agent: string; version: string; source: { kind: string; path?: string }; warning?: string }>;
    warnings: string[];
}

export const agentDetectionApi = {
    manifests: (): Promise<ManifestReport> => invoke("agent_detection_manifests"),
    reload: (): Promise<ManifestReport> => invoke("agent_detection_reload"),
    explain: (agentId: string): Promise<unknown> => invoke("agent_detection_explain", { agentId }),
};
