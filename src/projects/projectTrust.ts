import { activeProjectTaskInventoryMatches } from "../tasks/application";
import { projectControllerBridge } from "./controllerBridge";
import type { ProjectConfigLoadResult } from "./projectConfig";
import { trustProjectConfig } from "./projectConfigRuntime";

export type ValidProjectConfig = Extract<ProjectConfigLoadResult, { readonly status: "valid" }>;

export function activeProjectConfigMatches(project: string, expected: ValidProjectConfig, requireTaskInventory = false): boolean {
    const active = projectControllerBridge.getActiveSnapshot();
    const current = active?.config;
    return (
        active?.cwd === project &&
        current?.status === "valid" &&
        current.path === expected.path &&
        current.fingerprint === expected.fingerprint &&
        (!requireTaskInventory || activeProjectTaskInventoryMatches(project, expected.fingerprint))
    );
}

/**
 * Re-checks the on-disk config after approval: the user can take arbitrarily
 * long in the dialog, and the project must still be the one they approved.
 */
async function trustCurrentProjectConfig(project: string, expected: ValidProjectConfig, requireTaskInventory = false): Promise<boolean> {
    if (!activeProjectConfigMatches(project, expected, requireTaskInventory)) return false;
    if (!(await trustProjectConfig(expected))) return false;
    return activeProjectConfigMatches(project, expected, requireTaskInventory);
}

/** `if (!trusted) return;` in promise form, for command bodies that must stay void. */
export function whenTrusted(project: string, expected: ValidProjectConfig, requireTaskInventory: boolean, run: () => void): void {
    void trustCurrentProjectConfig(project, expected, requireTaskInventory).then((ok) => {
        if (ok) run();
    });
}
