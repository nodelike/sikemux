import type { useAuth } from "@clerk/react";
import type {
  Account,
  AccountDeletion,
  ApiError,
  DeviceList,
  ErrorCode,
  ErrorDetail,
  PendingHost,
} from "@sikemux/protocol";

import { config } from "./config.ts";

export class ApiProblem extends Error {
  readonly status: number;
  readonly code?: ErrorCode;
  /** With reverify_required: how recently the person must have signed in. */
  readonly reverifyWithinMinutes?: number;

  constructor(message: string, status: number, detail?: ErrorDetail) {
    super(message);
    this.status = status;
    this.code = detail?.code;
    this.reverifyWithinMinutes = detail?.reverifyWithinMinutes;
  }
}

type GetToken = ReturnType<typeof useAuth>["getToken"];

/** A token for the API, or a problem asking the person to sign in again to do `what`. */
export async function signedInToken(
  getToken: GetToken,
  what: string,
  { fresh = false } = {},
): Promise<string> {
  const token = await getToken(fresh ? { skipCache: true } : undefined);
  if (!token) throw new ApiProblem(`Sign in again to ${what}.`, 401);
  return token;
}

/** What went wrong, in words for the person. */
export function problemText(error: unknown): string {
  return error instanceof Error
    ? error.message
    : "Something went wrong; try again.";
}

/** A day the way the app writes one, like 12 November 2026. */
export function formatDay(iso: string, month: "short" | "long" = "long") {
  return new Date(iso).toLocaleDateString(undefined, {
    day: "numeric",
    month,
    year: "numeric",
  });
}

async function call(
  method: "GET" | "POST" | "DELETE",
  path: string,
  token: string,
): Promise<Response> {
  let response: Response;
  try {
    response = await fetch(`${config.apiUrl}${path}`, {
      method,
      headers: { authorization: `Bearer ${token}` },
    });
  } catch {
    throw new ApiProblem(
      "Can't reach Sikemux. Check your connection and try again.",
      0,
    );
  }
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as ApiError | null;
    throw new ApiProblem(
      body?.error.message ?? `The API answered ${response.status}.`,
      response.status,
      body?.error,
    );
  }
  return response;
}

const hostSignIn = (code: string) =>
  `/v1/host-sign-ins/${encodeURIComponent(code)}`;

export const api = {
  devices: async (token: string) =>
    (await (await call("GET", "/v1/devices", token)).json()) as DeviceList,
  removeDevice: async (token: string, key: string) => {
    await call("DELETE", `/v1/devices/${encodeURIComponent(key)}`, token);
  },
  account: async (token: string) =>
    (await (await call("GET", "/v1/account", token)).json()) as Account,
  restoreAccount: async (token: string) =>
    (await (
      await call("POST", "/v1/account/restore", token)
    ).json()) as Account,
  pendingHost: async (token: string, code: string) =>
    (await (await call("GET", hostSignIn(code), token)).json()) as PendingHost,
  approveHost: async (token: string, code: string) => {
    await call("POST", `${hostSignIn(code)}/approve`, token);
  },
  refuseHost: async (token: string, code: string) => {
    await call("DELETE", hostSignIn(code), token);
  },
  deleteAccount: async (token: string) =>
    (await (
      await call("DELETE", "/v1/account", token)
    ).json()) as AccountDeletion,
};
