import { invokeCommand as invoke } from "./invoke";

/** Another host the app joined, by its core's key. None, or null, is this host. */
export type HostId = string;

/** `args` for a command that goes to `host`, or to this host when there is none. */
export function onHost<Args extends Record<string, unknown>>(args: Args, host: HostId | null | undefined): Args & { host?: HostId } {
    return host ? { ...args, host } : args;
}

/** One of the core's methods, which every client reaches by name and the core alone decides who may call. */
export function coreCall<T>(method: string, params: unknown = null, host?: HostId | null): Promise<T> {
    return invoke<T>("core_call", onHost({ method, params }, host));
}

/** One of the core's requests: `op` names it and the rest are its fields. */
export type CoreRequest = { readonly op: string } & Readonly<Record<string, unknown>>;

/** Sends `request` to the core as is and answers with the core's reply. */
export function coreRequest<Reply = void>(request: CoreRequest, host?: HostId | null): Promise<Reply> {
    return invoke<Reply>("core_request", onHost({ request }, host));
}
