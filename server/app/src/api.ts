import type { useAuth } from "@clerk/react";
import { TRY_AGAIN } from "@sikemux/protocol/account";
import { ApiProblem, connectApi } from "@sikemux/protocol/api";

import { config } from "./config.ts";

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
  return error instanceof Error ? error.message : TRY_AGAIN;
}

/** A day the way the app writes one, like 12 November 2026. */
export function formatDay(iso: string, month: "short" | "long" = "long") {
  return new Date(iso).toLocaleDateString(undefined, {
    day: "numeric",
    month,
    year: "numeric",
  });
}

/** The API's routes, signed in with `token`. */
export const api = (token: string) =>
  connectApi({
    url: config.apiUrl,
    token: () => Promise.resolve(token),
    offline: "Can't reach Sikemux. Check your connection and try again.",
  });
