import { pluginsApi, type PluginFailure } from "../api/plugins";

export interface PluginStreamHandlers<Item> {
    readonly onItem: (item: Item) => void;
    readonly onEnd?: () => void;
    readonly onError?: (error: PluginFailure) => void;
}

export interface PluginStream {
    stop(): void;
}

export interface PluginBackend {
    call<Result>(method: string, params?: unknown): Promise<Result>;
    stream<Item>(method: string, params: unknown, handlers: PluginStreamHandlers<Item>): PluginStream;
    /** For a caller that keeps its own start and stop bookkeeping by stream id. */
    openStream<Item>(method: string, params: unknown, onItem: (item: Item) => void): Promise<number>;
    closeStream(streamId: number): Promise<void>;
}

export function isPluginFailure(error: unknown, category?: string): error is PluginFailure {
    if (typeof error !== "object" || error === null) return false;
    const candidate = error as Partial<PluginFailure>;
    if (typeof candidate.category !== "string" || typeof candidate.message !== "string") return false;
    return category === undefined || candidate.category === category;
}

export function createPluginBackend(pluginId: string): PluginBackend {
    return {
        call: <Result>(method: string, params: unknown = null) => pluginsApi.call<Result>(pluginId, method, params),

        openStream: <Item>(method: string, params: unknown, onItem: (item: Item) => void) =>
            pluginsApi.streamStart(pluginId, method, params, (event) => {
                if (event.kind === "item") onItem(event.value as Item);
            }),

        closeStream: (streamId: number) => pluginsApi.streamStop(streamId),

        stream<Item>(method: string, params: unknown, handlers: PluginStreamHandlers<Item>): PluginStream {
            let finished = false;
            let streamId: number | null = null;
            const finish = () => {
                finished = true;
            };
            pluginsApi
                .streamStart(pluginId, method, params, (event) => {
                    if (finished) return;
                    if (event.kind === "item") handlers.onItem(event.value as Item);
                    else if (event.kind === "end") {
                        finish();
                        handlers.onEnd?.();
                    } else {
                        finish();
                        handlers.onError?.(event.error);
                    }
                })
                .then(
                    (id) => {
                        streamId = id;
                        if (finished) void pluginsApi.streamStop(id);
                    },
                    (error: unknown) => {
                        if (finished) return;
                        finish();
                        handlers.onError?.(isPluginFailure(error) ? error : { category: "ipc", message: String(error) });
                    },
                );
            return {
                stop() {
                    if (finished) return;
                    finish();
                    if (streamId !== null) void pluginsApi.streamStop(streamId);
                },
            };
        },
    };
}
