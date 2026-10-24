import { isPluginFailure, type PluginBackend } from "../plugin-api/backend";
import { invalidate } from "../state/resources";

const IMAGES_KEPT = 400;

export interface CodeHostClient {
    /** A read or a change; one the rate limit turned away has the limit read again for its banner. */
    call<Result>(method: string, params?: unknown): Promise<Result>;
    /**
     * An avatar or a picture from the host as a `data:` address, since the window cannot load the host's images
     * itself. Each address is fetched once; one that failed is tried again next time.
     */
    image(url: string): Promise<string>;
}

/** What every code host plugin does the same way around its backend. */
export function createCodeHostClient(backend: PluginBackend): CodeHostClient {
    const images = new Map<string, Promise<string>>();

    const call = <Result>(method: string, params?: unknown): Promise<Result> =>
        backend.call<Result>(method, params).catch((error: unknown) => {
            if (isPluginFailure(error, "rate-limited")) invalidate((kind) => kind === "host.rateLimit");
            throw error;
        });

    return {
        call,

        image(url: string): Promise<string> {
            const known = images.get(url);
            if (known) return known;
            const fetched = call<string>("image", { url });
            images.set(url, fetched);
            fetched.catch(() => images.delete(url));
            if (images.size > IMAGES_KEPT) {
                const oldest = images.keys().next().value;
                if (oldest !== undefined) images.delete(oldest);
            }
            return fetched;
        },
    };
}
