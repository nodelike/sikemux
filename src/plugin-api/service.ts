import { invalidate, resource, useResource, type ResourceDef } from "../state/resources";
import { resourceKind, type PluginIdentity } from "./identity";
import { signInOf, type PluginSignIn } from "./signInState";

export interface PluginResources {
    resource<Args extends unknown[], T>(name: string, fetch: (...args: Args) => Promise<T>, staleAfterMs?: number): ResourceDef<Args, T>;
    /** Reads again everything the plugin has cached, or only `these` when any are named. */
    refresh(...these: readonly { readonly kind: string }[]): void;
    /** Whether a cached read is one of the plugin's. */
    owns(kind: string): boolean;
}

/** The reads a plugin caches, each named under the plugin's id. */
export function pluginResources(plugin: PluginIdentity): PluginResources {
    const prefix = resourceKind(plugin.id, "");
    const owns = (kind: string) => kind.startsWith(prefix);
    return {
        resource: (name, fetch, staleAfterMs) => resource({ kind: resourceKind(plugin.id, name), fetch, staleAfterMs }),
        refresh: (...these) => invalidate((kind) => (these.length === 0 ? owns(kind) : these.some((each) => each.kind === kind))),
        owns,
    };
}

/** What a service's `status` says about the sign-in it holds. */
export interface ServiceStatus {
    readonly configured: boolean;
    readonly authFailed: boolean;
    readonly message: string | null;
}

export interface Service<Status extends ServiceStatus> extends PluginIdentity, PluginResources {
    /** Read again whenever a call finds the person signed out. */
    readonly statusR: ResourceDef<[], Status>;
    useSignIn(): PluginSignIn;
}

/** A plugin's cached reads of a service the person signs in to, starting with whether they are. */
export function defineService<Status extends ServiceStatus>(plugin: PluginIdentity, status: () => Promise<Status>): Service<Status> {
    const resources = pluginResources(plugin);
    const statusR = resources.resource("status", status, 60_000);
    return {
        ...plugin,
        ...resources,
        statusR,
        useSignIn: function useServiceSignIn() {
            return signInOf(useResource(statusR));
        },
    };
}
