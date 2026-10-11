import type {
  Account,
  AccountDeletion,
  ApiError,
  DeviceList,
  ErrorCode,
} from "@sikemux/protocol";

import { config } from "./config.ts";

export class ApiProblem extends Error {
  readonly status: number;
  readonly code?: ErrorCode;

  constructor(message: string, status: number, code?: ErrorCode) {
    super(message);
    this.status = status;
    this.code = code;
  }
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
      body?.error.code,
    );
  }
  return response;
}

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
  deleteAccount: async (token: string) =>
    (await (
      await call("DELETE", "/v1/account", token)
    ).json()) as AccountDeletion,
};
