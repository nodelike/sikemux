import { coreCall } from "./core";
import { cancelCoreStream, coreStream } from "./coreStream";
import { invokeCommand as invoke } from "./invoke";

export interface PluginManifest {
    readonly id: string;
    readonly name: string;
    readonly version: string;
    readonly sikemux: string;
}

export interface PluginFailure {
    readonly category: string;
    readonly message: string;
    readonly status?: number;
    readonly plugin?: string;
}

export type PluginStreamEvent =
    { readonly kind: "item"; readonly value: unknown } | { readonly kind: "end" } | { readonly kind: "error"; readonly error: PluginFailure };

export const pluginsApi = {
    manifests: () => coreCall<PluginManifest[]>("pluginManifests"),

    call: <T>(plugin: string, method: string, params: unknown) => invoke<T>("plugin_call", { plugin, method, params }),

    streamStart: async (plugin: string, method: string, params: unknown, onEvent: (event: PluginStreamEvent) => void) => {
        const { stream, done } = await coreStream("pluginStream", { plugin, method, params }, (value) => onEvent({ kind: "item", value }));
        done.then(
            () => onEvent({ kind: "end" }),
            (error: unknown) => onEvent({ kind: "error", error: error as PluginFailure }),
        );
        return stream;
    },

    streamStop: (streamId: number) => cancelCoreStream(streamId),
};
