import { apiClient, type ApiClient, type Method } from "../../server/protocol/generated/client";
import type { ApiError, ErrorCode, ErrorDetail } from "../../server/protocol/generated/types";
import { english, type Say } from "../lib/say";

/** Why a request to the API failed, in words for the person. */
export class ApiProblem extends Error {
    /** The API's status, or undefined when it could not be reached at all. */
    readonly status: number | undefined;
    readonly code: ErrorCode | undefined;
    /** Only with reverify_required: how recently the person must have proved who they are. */
    readonly reverifyWithinMinutes: number | undefined;

    constructor(message: string, status?: number, detail?: ErrorDetail) {
        super(message);
        this.status = status;
        this.code = detail?.code;
        this.reverifyWithinMinutes = detail?.reverifyWithinMinutes;
    }

    /** Nothing changed on the server: it was out of reach, or failed on its side. */
    get unreachable(): boolean {
        return this.status === undefined || this.status >= 500;
    }
}

export interface Connection {
    url: string;
    /** The sign-in to send, for routes that need one; null when the person must sign in again. */
    token?: () => Promise<string | null>;
    /** What to tell the person when the API cannot be reached. */
    offline: string;
    /** How long a request may take before it counts as out of reach. */
    timeoutMs?: number;
    say?: Say;
}

export async function bearer(token: () => Promise<string | null>): Promise<string> {
    const signedIn = await token();
    if (!signedIn) throw new ApiProblem("Sign in again.", 401);
    return signedIn;
}

/** The problem an answer with `status` describes in `body`, the API's error as JSON when it sent one. */
export function answerProblem(status: number, body: unknown, say: Say = english): ApiProblem {
    const detail = (body as Partial<ApiError> | null)?.error;
    return new ApiProblem(detail?.message ?? say("The accounts server answered {status}.", { status }), status, detail);
}

/** The API's routes over fetch, signed in with `token` when given, every failure an ApiProblem. */
export function connectApi({ url, token, offline, timeoutMs, say }: Connection): ApiClient {
    return apiClient(async <Answer>(method: Method, path: string, body?: unknown): Promise<Answer> => {
        let response: Response;
        const abort = new AbortController();
        const timer = timeoutMs === undefined ? undefined : setTimeout(() => abort.abort(), timeoutMs);
        try {
            response = await fetch(`${url}${path}`, {
                method,
                headers: {
                    ...(token ? { authorization: `Bearer ${await bearer(token)}` } : {}),
                    ...(body === undefined ? {} : { "content-type": "application/json" }),
                },
                ...(body === undefined ? {} : { body: JSON.stringify(body) }),
                signal: abort.signal,
            });
        } catch (error) {
            if (error instanceof ApiProblem) throw error;
            throw new ApiProblem(offline);
        } finally {
            clearTimeout(timer);
        }
        if (!response.ok) throw answerProblem(response.status, await response.json().catch(() => null), say);
        return (response.status === 204 ? undefined : await response.json()) as Answer;
    });
}
