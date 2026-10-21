import { coreCall } from "../api/core";
import { dirname } from "../lib/paths";
import type { ProjectConfigLoadResult } from "./projectConfig";
import { confirmDialog, type ConfirmRequest } from "../state/dialog";

/**
 * Asks the person to trust the file unless the host already does, and keeps
 * their yes on the host until the file changes.
 */
export async function trustProjectConfig(
    result: Extract<ProjectConfigLoadResult, { status: "valid" }>,
    ask: (request: ConfirmRequest) => Promise<boolean> = confirmDialog,
): Promise<boolean> {
    if (result.trusted || !result.trustAsk) return true;
    if (!(await ask(result.trustAsk))) return false;
    await coreCall("trustProject", { project: dirname(result.path), fingerprint: result.fingerprint });
    return true;
}
