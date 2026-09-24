import { describe, expect, it } from "vitest";
import {
    agentLaunchArgs,
    isDangerousPermissionMode,
    normalizeAgentEffort,
    normalizePermissionMode,
    permissionArgs,
    supportedEfforts,
    supportedPermissionModes,
} from "./agentLaunch";

describe("agent launch policy", () => {
    it("maps Codex Normal and YOLO modes to explicit flags", () => {
        expect(permissionArgs("codex", "workspace-write")).toEqual(["--sandbox", "workspace-write"]);
        expect(permissionArgs("codex", "bypass")).toEqual(["--dangerously-bypass-approvals-and-sandbox"]);
        expect(permissionArgs("codex", "read-only")).toEqual(["--sandbox", "workspace-write"]);
        expect(permissionArgs("codex", "full-access")).toEqual(["--sandbox", "workspace-write"]);
    });

    it("maps Claude Normal and YOLO modes without reviving legacy choices", () => {
        expect(permissionArgs("claude", "workspace-write")).toEqual(["--permission-mode", "acceptEdits"]);
        expect(permissionArgs("claude", "bypass")).toEqual(["--dangerously-skip-permissions"]);
        expect(permissionArgs("claude", "read-only")).toEqual(["--permission-mode", "acceptEdits"]);
        expect(permissionArgs("claude", "full-access")).toEqual(["--permission-mode", "acceptEdits"]);
    });

    it("marks only bypass dangerous", () => {
        expect(isDangerousPermissionMode("full-access")).toBe(false);
        expect(isDangerousPermissionMode("bypass")).toBe(true);
    });

    it("presents only Normal and YOLO, without inventing unsupported provider flags", () => {
        expect(supportedPermissionModes("codex")).toEqual(["workspace-write", "bypass"]);
        expect(supportedPermissionModes("hermes")).toEqual(["workspace-write", "bypass"]);
        expect(supportedPermissionModes("omp")).toEqual(["workspace-write", "bypass"]);
        expect(supportedPermissionModes("grok")).toEqual(["workspace-write", "bypass"]);
        expect(supportedPermissionModes("pi")).toEqual(["workspace-write"]);
        expect(normalizePermissionMode("opencode", "read-only")).toBe("workspace-write");
        expect(permissionArgs("pi", "read-only")).toEqual([]);
    });

    it("builds Claude model, effort, permission, and resume arguments", () => {
        expect(
            agentLaunchArgs("claude", {
                resumeId: "session-1",
                permissionMode: "workspace-write",
                model: " sonnet ",
                effort: "high",
            }),
        ).toEqual(["--model", "sonnet", "--effort", "high", "--permission-mode", "acceptEdits", "--resume", "session-1"]);
    });

    it("places Codex resume first and passes reasoning as a config override", () => {
        expect(
            agentLaunchArgs("codex", {
                resumeId: "session-2",
                permissionMode: "workspace-write",
                model: "gpt-5.6-codex",
                effort: "xhigh",
            }),
        ).toEqual(["resume", "--model", "gpt-5.6-codex", "--config", 'model_reasoning_effort="xhigh"', "--sandbox", "workspace-write", "session-2"]);
    });

    it("uses only flags supported by each local interactive CLI", () => {
        expect(agentLaunchArgs("hermes", { model: "anthropic/claude-sonnet-4.6", effort: "ultra" })).toEqual([
            "--model",
            "anthropic/claude-sonnet-4.6",
            "--reasoning",
            "ultra",
            "--tui",
        ]);
        expect(agentLaunchArgs("pi", { model: "anthropic/claude-sonnet-4.6", effort: "ultra" })).toEqual([
            "--model",
            "anthropic/claude-sonnet-4.6",
            "--thinking",
            "max",
        ]);
        expect(agentLaunchArgs("opencode", { model: "openai/gpt-5", effort: "high" })).toEqual(["--model", "openai/gpt-5"]);
        expect(
            agentLaunchArgs("omp", {
                resumeId: "/sessions/omp.jsonl",
                model: "openai-codex/gpt-5.6-sol",
                effort: "xhigh",
                permissionMode: "bypass",
            }),
        ).toEqual(["--model", "openai-codex/gpt-5.6-sol", "--thinking", "xhigh", "--approval-mode", "yolo", "--resume", "/sessions/omp.jsonl"]);
        expect(
            agentLaunchArgs("grok", {
                resumeId: "018f0000-0000-7000-8000-000000000000",
                model: "grok-4.5",
                effort: "max",
                permissionMode: "bypass",
            }),
        ).toEqual([
            "--model",
            "grok-4.5",
            "--reasoning-effort",
            "max",
            "--permission-mode",
            "bypassPermissions",
            "--resume",
            "018f0000-0000-7000-8000-000000000000",
        ]);
    });

    it("reports effort support without inventing provider capabilities", () => {
        expect(supportedEfforts("claude")).toEqual(["low", "medium", "high", "xhigh", "max"]);
        expect(supportedEfforts("hermes")).toContain("ultra");
        expect(supportedEfforts("opencode")).toEqual([]);
        expect(supportedEfforts("omp")).toEqual(["off", "minimal", "low", "medium", "high", "xhigh", "max"]);
        expect(supportedEfforts("grok")).toEqual(["none", "minimal", "low", "medium", "high", "xhigh", "max"]);
        expect(normalizeAgentEffort("codex", "ultra")).toBe("max");
        expect(normalizeAgentEffort("opencode", "high")).toBeUndefined();
    });
});
