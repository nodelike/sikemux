import type { ResourceHandle } from "../../plugin-api/resources";
import type { AwsIdentity } from "./api";

export type AwsAuthState =
    | { kind: "no-profile" }
    | { kind: "checking"; profile: string }
    | { kind: "authed"; profile: string; identity: AwsIdentity }
    | { kind: "expired"; profile: string; identity: AwsIdentity }
    | { kind: "no-credentials"; profile: string; identity: AwsIdentity }
    | { kind: "cli-missing"; profile: string; identity: AwsIdentity }
    | { kind: "error"; profile: string; identity: AwsIdentity | null; message: string };

export function deriveAuthState(profile: string | null, handle: ResourceHandle<AwsIdentity>): AwsAuthState {
    if (!profile) return { kind: "no-profile" };
    const id = handle.data;
    if (!id) {
        if (handle.status === "error") {
            return {
                kind: "error",
                profile,
                identity: null,
                message: handle.error ?? "unknown error",
            };
        }
        return { kind: "checking", profile };
    }
    switch (id.status) {
        case "authed":
            return { kind: "authed", profile, identity: id };
        case "expired":
            return { kind: "expired", profile, identity: id };
        case "no-credentials":
            return { kind: "no-credentials", profile, identity: id };
        case "cli-missing":
            return { kind: "cli-missing", profile, identity: id };
        case "error":
        default:
            return {
                kind: "error",
                profile,
                identity: id,
                message: id.message ?? "unknown error",
            };
    }
}

export function needsAuth(s: AwsAuthState): boolean {
    return s.kind === "expired" || s.kind === "no-credentials" || s.kind === "error" || s.kind === "cli-missing";
}
