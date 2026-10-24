import { coreCall } from "./core";
import { getIpcTransport } from "./transport";

/** A part of an answer the core sends in parts, as the app passes it on. */
interface StreamPart {
    readonly stream: number;
    readonly items: unknown[];
    readonly done: boolean;
    readonly error?: unknown;
}

const STREAM_EVENT = "core_stream";
let nextStream = 1;

/**
 * Starts the core's `method`, handing each part it sends to `onItem`. The stream is named here, so its parts are
 * listened for before the core can send any; `done` settles when the last part arrives, failing with the stream's
 * error, `{category, message}` and whatever its source adds.
 */
export async function coreStream(
    method: string,
    params: Record<string, unknown>,
    onItem: (item: unknown) => void,
): Promise<{ readonly stream: number; readonly done: Promise<void> }> {
    const stream = nextStream++;
    const controller = new AbortController();
    let finish: () => void = () => {};
    let fail: (error: unknown) => void = () => {};
    const done = new Promise<void>((resolve, reject) => {
        finish = resolve;
        fail = reject;
    });
    await getIpcTransport().subscribe<StreamPart>(
        STREAM_EVENT,
        ({ payload }) => {
            if (payload.stream !== stream) return;
            for (const item of payload.items) onItem(item);
            if (!payload.done) return;
            controller.abort();
            if (payload.error) fail(payload.error);
            else finish();
        },
        { signal: controller.signal },
    );
    try {
        await coreCall(method, { ...params, stream });
    } catch (error) {
        controller.abort();
        throw error;
    }
    return { stream, done };
}

/** Stops a stream the core is still sending. */
export function cancelCoreStream(stream: number): Promise<void> {
    return coreCall("streamCancel", { stream });
}
