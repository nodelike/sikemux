import { Channel } from "@tauri-apps/api/core";
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
    manifests: () => invoke<PluginManifest[]>("plugin_manifests"),

    setDisabled: (ids: readonly string[]) => invoke<void>("plugin_set_disabled", { ids }),

    call: <T>(plugin: string, method: string, params: unknown) => invoke<T>("plugin_call", { plugin, method, params }),

    streamStart: (plugin: string, method: string, params: unknown, onEvent: (event: PluginStreamEvent) => void) => {
        const channel = new Channel<PluginStreamEvent>();
        channel.onmessage = onEvent;
        return invoke<number>("plugin_stream_start", { plugin, method, params, onEvent: channel });
    },

    streamStop: (streamId: number) => invoke<void>("plugin_stream_stop", { streamId }),
};
