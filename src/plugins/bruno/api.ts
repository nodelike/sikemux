import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { BRUNO_PLUGIN } from "./kinds";
import type { BruRequest } from "./lib/types";
import type { BruRunResult } from "./types.generated";

const backend = createPluginBackend(BRUNO_PLUGIN.id);

export type BruBodyWire =
    | { kind: "none" }
    | { kind: "raw"; content_type: string | null; data: string }
    | { kind: "file"; path: string; content_type: string | null }
    | { kind: "form"; fields: [string, string][] }
    | { kind: "multipart"; fields: { name: string; value: string; is_file: boolean }[] };

export interface BruSendRequest {
    method: string;
    url: string;
    headers: [string, string][];
    body: BruBodyWire;
    timeout_ms: number;
    skip_tls_verify: boolean;
    trust: {
        allow_private_network: boolean;
        allow_file_read: boolean;
        allow_insecure_tls: boolean;
        file_root: string | null;
    };
}

export interface BruSendResponse {
    status: number;
    status_text: string;
    headers: [string, string][];
    body: string;
    is_binary: boolean;
    size_bytes: number;
    duration_ms: number;
}

export const brunoApi = {
    /** Writes the request to its file, inside the collection `root`. */
    saveRequest: (root: string, path: string, request: BruRequest) => backend.call<void>("saveRequest", { root, path, request }),
    /** Makes a request named `name` in `folder`, and says where its file is. */
    newRequest: (root: string, folder: string, name: string) => backend.call<string>("newRequest", { root, folder, name }),
    newFolder: (root: string, folder: string, name: string) => backend.call<string>("newFolder", { root, folder, name }),
    /** Renames a request and its file, keeping its unsaved edits when given, and says where it is now. */
    renameRequest: (root: string, path: string, name: string, request?: BruRequest) =>
        backend.call<string>("renameRequest", { root, path, name, request }),
    deletePath: (root: string, path: string) => backend.call<void>("deletePath", { root, path }),
    /** Runs the request as edited, with its scripts when the collection is trusted, and says how it went. */
    run: (asked: {
        root: string;
        path: string;
        request?: BruRequest;
        env?: string;
        secrets: Record<string, string>;
        runtime: Record<string, string>;
        trusted: boolean;
    }) => backend.call<BruRunResult>("run", asked),
    /** The host names the plugin in front of every error; a request's error reads better without it. */
    async send(req: BruSendRequest): Promise<BruSendResponse> {
        try {
            return await backend.call<BruSendResponse>("send", { req });
        } catch (error) {
            if (!isPluginFailure(error)) throw error;
            throw new Error(error.message.replace(`${BRUNO_PLUGIN.id}: `, ""), { cause: error });
        }
    },
};
