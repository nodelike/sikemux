import type { MarkdownOptions, MdElement } from "../markdown/types";
import { invokeCommand as invoke } from "./invoke";

export interface MarkdownRequest {
    readonly text: string;
    readonly options: MarkdownOptions;
    /** How many leading blocks the caller already holds and does not need again. */
    readonly skip: number;
}

interface Waiting {
    readonly request: MarkdownRequest;
    readonly resolve: (blocks: MdElement[]) => void;
    readonly reject: (error: unknown) => void;
}

let waiting: Waiting[] = [];

function send(): void {
    const batch = waiting;
    waiting = [];
    invoke<MdElement[][]>("markdown_parse", { requests: batch.map((item) => item.request) }).then(
        (results) => batch.forEach((item, index) => item.resolve(results[index] ?? [])),
        (error: unknown) => batch.forEach((item) => item.reject(error)),
    );
}

export const markdownApi = {
    /** Requests made in the same task travel together, so a transcript opening costs one round trip. */
    parse(request: MarkdownRequest): Promise<MdElement[]> {
        if (waiting.length === 0) queueMicrotask(send);
        return new Promise((resolve, reject) => waiting.push({ request, resolve, reject }));
    },
};
