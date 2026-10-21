import { pluginsApi, type PluginFailure } from "../api/plugins";
import { errorText } from "../lib/format";
import { invalidate } from "../state/resources";
import { resourceKind } from "./identity";

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
    /** For a stream item that says the person is signed out: reads the sign-in status again, as a failed call does. */
    signedOut(): void;
}

export interface PluginBackendOptions {
    /**
     * Cached answers besides the plugin's own `status` that say whether the person is signed in. A call that finds
     * them signed out has only these read again, so the pane turns to its sign-in form. Re-reading every answer would
     * re-read the one that just failed, which fails again, forever.
     */
    readonly alsoStatus?: readonly string[];
    /** The plugin's own failure categories that also mean the person has to sign in again. */
    readonly signedOutCategories?: readonly string[];
}

export function isPluginFailure(error: unknown, category?: string): error is PluginFailure {
    if (typeof error !== "object" || error === null) return false;
    const candidate = error as Partial<PluginFailure>;
    if (typeof candidate.category !== "string" || typeof candidate.message !== "string") return false;
    return category === undefined || candidate.category === category;
}

/** A request that never reached the service, such as when the Mac is offline. */
export function isOfflineFailure(error: unknown): boolean {
    return isPluginFailure(error) && (error.category === "offline" || error.category === "timeout");
}

/** A sign-in the service no longer takes. A 403 is not one: the person is signed in, just not allowed. */
export function isSignedOutFailure(error: unknown, alsoSignedOut: readonly string[] = []): boolean {
    if (!isPluginFailure(error)) return false;
    if (error.category === "auth" || error.category === "unconfigured") return true;
    if (error.category === "http" && error.status === 401) return true;
    return alsoSignedOut.includes(error.category);
}

export function createPluginBackend(pluginId: string, options: PluginBackendOptions = {}): PluginBackend {
    const { alsoStatus = [], signedOutCategories } = options;
    const statusKinds = [resourceKind(pluginId, "status"), ...alsoStatus];
    const signedOut = () => invalidate((kind) => statusKinds.includes(kind));
    const noticeFailure = (error: unknown) => {
        if (isSignedOutFailure(error, signedOutCategories)) signedOut();
    };

    return {
        signedOut,

        call: <Result>(method: string, params: unknown = null) =>
            pluginsApi.call<Result>(pluginId, method, params).catch((error: unknown) => {
                noticeFailure(error);
                throw error;
            }),

        stream<Item>(method: string, params: unknown, handlers: PluginStreamHandlers<Item>): PluginStream {
            let finished = false;
            let streamId: number | null = null;
            const fail = (error: PluginFailure) => {
                finished = true;
                noticeFailure(error);
                handlers.onError?.(error);
            };
            pluginsApi
                .streamStart(pluginId, method, params, (event) => {
                    if (finished) return;
                    if (event.kind === "item") handlers.onItem(event.value as Item);
                    else if (event.kind === "end") {
                        finished = true;
                        handlers.onEnd?.();
                    } else fail(event.error);
                })
                .then(
                    (id) => {
                        streamId = id;
                        if (finished) void pluginsApi.streamStop(id);
                    },
                    (error: unknown) => {
                        if (!finished) fail(isPluginFailure(error) ? error : { category: "ipc", message: errorText(error) });
                    },
                );
            return {
                stop() {
                    if (finished) return;
                    finished = true;
                    if (streamId !== null) void pluginsApi.streamStop(streamId);
                },
            };
        },
    };
}

export interface BrowserSignIn<Result> {
    /** Resolves with what signing in gave once the browser comes back, and rejects if it never does. */
    readonly done: Promise<Result>;
    cancel(): void;
}

/** What a browser sign-in rejects with when the person cancels it, which is not a failure to show. */
export class SignInCancelled extends Error {
    constructor() {
        super("cancelled");
    }
}

/**
 * Waits for the browser to come back from a service's sign-in page. The stream sends the page as `{ url }`, which
 * `openPage` is handed to show, and then what signing in gave.
 */
export function browserSignIn<Result>(
    backend: PluginBackend,
    method: string,
    openPage: (url: string) => void,
    params: unknown = {},
): BrowserSignIn<Result> {
    let cancel = () => {};
    const done = new Promise<Result>((resolve, reject) => {
        let answer: { result: Result } | null = null;
        const stream = backend.stream<unknown>(method, params, {
            onItem: (item) => {
                const url = (item as { url?: unknown } | null)?.url;
                if (typeof url === "string") openPage(url);
                else answer = { result: item as Result };
            },
            onEnd: () => (answer ? resolve(answer.result) : reject(new Error("the sign-in ended before it finished"))),
            onError: reject,
        });
        cancel = () => {
            stream.stop();
            reject(new SignInCancelled());
        };
    });
    return { done, cancel: () => cancel() };
}
