import type { ResourceHandle } from "../state/resources";

/** "failed" is a sign-in the service stopped taking, or a check that could not reach it; "signed-out" is never having signed in. */
export type PluginSignIn = "signed-in" | "checking" | "signed-out" | "failed";

export function signInOf(status: ResourceHandle<{ readonly configured: boolean; readonly authFailed: boolean }>): PluginSignIn {
    if (!status.data) return status.status === "error" ? "failed" : "checking";
    if (!status.data.configured) return "signed-out";
    return status.data.authFailed ? "failed" : "signed-in";
}
