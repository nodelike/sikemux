import type { Services } from "../app.ts";
import type { MacSignIn } from "@sikemux/protocol";
import { createHash } from "node:crypto";
import { Hono } from "hono";

import { checkStanding, type AuthEnv } from "../auth.ts";
import { ApiFailure, readBody, unauthorized, type Env } from "../http.ts";
import { clientAddress, limit } from "../limits.ts";
import { issueChallenge, macSignInChallenge } from "./challenge.ts";
import { registerDevice, toDevice } from "./register.ts";

const TIMEOUT_MS = 10_000;
/** Longer than Clerk keeps a code good for, so a code is never traded twice through here. */
const CODE_MEMORY_MS = 15 * 60_000;

type Fetch = (
  input: string | URL | Request,
  init?: RequestInit,
) => Promise<Response>;

/** Trades the code a Mac's browser sign-in handed back for Clerk's access token. */
export interface MacSignInExchange {
  /** The access token, or null when Clerk refuses the code. Throws when Clerk can't be reached. */
  accessToken(signIn: MacSignIn): Promise<string | null>;
}

export function clerkMacSignIn(
  issuer: string,
  clientId: string,
  fetcher: Fetch = fetch,
): MacSignInExchange {
  const post = (path: string, form: Record<string, string>) =>
    fetcher(new URL(path, issuer), {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ client_id: clientId, ...form }).toString(),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
  return {
    async accessToken({ code, codeVerifier, redirectUri }) {
      const response = await post("/oauth/token", {
        grant_type: "authorization_code",
        code,
        code_verifier: codeVerifier,
        redirect_uri: redirectUri,
      });
      if (response.status >= 500)
        throw new Error(`Clerk answered ${response.status} to /oauth/token`);
      const tokens = (await response.json().catch(() => null)) as {
        access_token?: unknown;
        refresh_token?: unknown;
      } | null;
      if (typeof tokens?.refresh_token === "string")
        await post("/oauth/token/revoke", {
          token: tokens.refresh_token,
          token_type_hint: "refresh_token",
        })
          .then((revoked) => revoked.body?.cancel())
          .catch(() => undefined);
      return response.ok && typeof tokens?.access_token === "string"
        ? tokens.access_token
        : null;
    },
  };
}

function refused(): ApiFailure {
  return unauthorized("The sign-in didn't go through. Sign in again.");
}

/**
 * Adding a Mac's host from its browser sign-in. The Mac hands over the code its browser got, the
 * API trades it at Clerk itself, so a token the Mac kept or refreshed never adds a host, and the
 * Mac signs the challenge that comes back with its key. It holds no sign-in token afterwards.
 */
export function macSignInRoutes({
  database: { db },
  verifier,
  limiter,
  macSignIn,
}: Pick<Services, "database" | "verifier" | "limiter" | "macSignIn">) {
  const perAddress = (name: string, perMinute: number) =>
    limit<Env>(limiter, name, perMinute, clientAddress);
  const traded = new Map<string, number>();
  const firstTrade = (code: string) => {
    const now = Date.now();
    for (const [seen, until] of traded) if (until <= now) traded.delete(seen);
    const hash = createHash("sha256").update(code).digest("hex");
    if (traded.has(hash)) return false;
    traded.set(hash, now + CODE_MEMORY_MS);
    return true;
  };

  return new Hono<AuthEnv>()
    .post(
      "/",
      perAddress("mac-sign-in", 10),
      limit<Env>(limiter, "mac-sign-in-all", 120, () => "all"),
      async (c) => {
        const signIn = await readBody(c, "MacSignIn");
        if (!firstTrade(signIn.code)) throw refused();
        const token = await macSignIn.accessToken(signIn);
        if (!token) throw refused();
        const identity = await verifier
          .verify(token)
          .catch((error: unknown) => {
            throw error instanceof ApiFailure && error.status === 401
              ? refused()
              : error;
          });
        if (identity.via !== "mac" || !identity.issuedAt) throw refused();
        await checkStanding(db, identity);
        const { nonce, expiresAt } = await issueChallenge(db, identity.userId, {
          macSignedInAt: identity.issuedAt,
        });
        return c.json({ account: identity.userId, nonce, expiresAt });
      },
    )
    .post("/finish", perAddress("mac-sign-in-finish", 10), async (c) => {
      const { nonce, ...host } = await readBody(c, "MacSignInFinish");
      const { userId, signedInAt } = await macSignInChallenge(db, nonce);
      const { row, added } = await registerDevice(
        db,
        { userId, via: "mac", issuedAt: signedInAt, macSignIn: true },
        { ...host, role: "host", nonce },
      );
      c.get("log").info(
        { key: row.key, added },
        added ? "added a Mac" : "signed a Mac in again",
      );
      return c.json(toDevice(row), added ? 201 : 200);
    });
}
