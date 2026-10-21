import { apiClient, type ApiClient, type Method } from "../generated/client.ts";
import type { ApiError, ErrorCode, ErrorDetail } from "../generated/types.ts";

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
  /** The sign-in to send; null when the person must sign in again. */
  token: () => Promise<string | null>;
  /** What to tell the person when the API cannot be reached. */
  offline: string;
}

/** The API's routes over fetch, signed in with `token`, every failure an ApiProblem. */
export function connectApi({ url, token, offline }: Connection): ApiClient {
  return apiClient(
    async <Answer>(
      method: Method,
      path: string,
      body?: unknown,
    ): Promise<Answer> => {
      let response: Response;
      try {
        const bearer = await token();
        if (!bearer) throw new ApiProblem("Sign in again.", 401);
        response = await fetch(`${url}${path}`, {
          method,
          headers: {
            authorization: `Bearer ${bearer}`,
            ...(body === undefined
              ? {}
              : { "content-type": "application/json" }),
          },
          ...(body === undefined ? {} : { body: JSON.stringify(body) }),
        });
      } catch (error) {
        if (error instanceof ApiProblem) throw error;
        throw new ApiProblem(offline);
      }
      if (!response.ok) {
        const failure = (await response
          .json()
          .catch(() => null)) as Partial<ApiError> | null;
        throw new ApiProblem(
          failure?.error?.message ??
            `The accounts server answered ${response.status}.`,
          response.status,
          failure?.error,
        );
      }
      return (
        response.status === 204 ? undefined : await response.json()
      ) as Answer;
    },
  );
}
