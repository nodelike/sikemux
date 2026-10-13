import type { PluginStreamEvent } from "../api/plugins";
import type { PluginStreamHandlers } from "./backend";

interface Fake {
    call: (method: string, params: unknown) => unknown;
    /** Drives one stream through `handlers`; whatever it returns with a `stop` is told when the stream is stopped. */
    stream: (method: string, params: unknown, handlers: PluginStreamHandlers<unknown>) => unknown;
}

/** For tests: the app's plugin transport, answered by `fake` in the shape a plugin backend sees, as `vi.mock("…/api/plugins")`. */
export function fakeTransport(fake: Fake) {
    const streams = new Map<number, unknown>();
    let nextId = 0;
    return {
        pluginsApi: {
            call: (_plugin: string, method: string, params: unknown) => Promise.resolve().then(() => fake.call(method, params)),
            streamStart: (_plugin: string, method: string, params: unknown, onEvent: (event: PluginStreamEvent) => void) => {
                const id = ++nextId;
                streams.set(
                    id,
                    fake.stream(method, params, {
                        onItem: (value) => onEvent({ kind: "item", value }),
                        onEnd: () => onEvent({ kind: "end" }),
                        onError: (error) => onEvent({ kind: "error", error }),
                    }),
                );
                return Promise.resolve(id);
            },
            streamStop: (id: number) => {
                (streams.get(id) as { stop?: () => void } | undefined)?.stop?.();
                return Promise.resolve();
            },
        },
    };
}
