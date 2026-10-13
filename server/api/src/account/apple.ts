import type { PushApp } from "@sikemux/protocol";
import type { Kysely } from "kysely";

import {
  readAppleKeySettings,
  signAppleToken,
  type AppleKey,
} from "../apple-key.ts";
import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import { APNS_TOPICS } from "../push/apns.ts";
import type { ClerkBackend } from "./clerk.ts";

/** The Services ID the web app signs in with Apple as, through Clerk. Only production has one. */
export const APPLE_WEB_CLIENT_ID = "com.nodelike.sikemux.signin";

const APPLE = "https://appleid.apple.com";
const TIMEOUT_MS = 10_000;
/** How long a client secret is good for; each one is made for a single exchange. */
const SECRET_LIFE_SECONDS = 5 * 60;

type Fetch = (
  input: string | URL | Request,
  init?: RequestInit,
) => Promise<Response>;

/** The two ways the API asks Apple to forget a person's Sign in with Apple. */
export interface AppleSignIn {
  /** Trades an authorization code the phone just got for a refresh token, and revokes it. */
  revokeCode(code: string, clientId: string): Promise<void>;
  revokeAccessToken(token: string, clientId: string): Promise<void>;
}

/** Reads APPLE_SIGNIN_KEY_FILE, the Sign in with Apple .p8 key, with its APPLE_SIGNIN_KEY_ID and APPLE_SIGNIN_TEAM_ID. */
export function readAppleSignIn(
  env: NodeJS.ProcessEnv,
  problems: string[],
): AppleKey | null {
  return readAppleKeySettings(env, "APPLE_SIGNIN", problems);
}

/** The client secret Apple's token and revoke endpoints take, signed with the team's key. */
export function clientSecret(
  key: AppleKey,
  clientId: string,
  nowMs: number,
): string {
  const iat = Math.floor(nowMs / 1000);
  return signAppleToken(key, {
    iss: key.teamId,
    iat,
    exp: iat + SECRET_LIFE_SECONDS,
    aud: APPLE,
    sub: clientId,
  });
}

/** Apple's error codes are short snake_case words; anything else in the body stays out of logs. */
async function reasonOf(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as {
    error?: unknown;
  } | null;
  return typeof body?.error === "string" && /^[a-z_]{1,64}$/.test(body.error)
    ? ` (${body.error})`
    : "";
}

export function appleSignIn(
  key: AppleKey,
  {
    fetcher = fetch,
    now = Date.now,
  }: { fetcher?: Fetch; now?: () => number } = {},
): AppleSignIn {
  const post = async (
    path: string,
    clientId: string,
    form: Record<string, string>,
  ) => {
    const response = await fetcher(`${APPLE}${path}`, {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        client_id: clientId,
        client_secret: clientSecret(key, clientId, now()),
        ...form,
      }).toString(),
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    if (!response.ok)
      throw new Error(
        `Apple answered ${response.status} to ${path}${await reasonOf(response)}`,
      );
    return response;
  };
  const revoke = async (
    token: string,
    hint: "refresh_token" | "access_token",
    clientId: string,
  ) => {
    const response = await post("/auth/revoke", clientId, {
      token,
      token_type_hint: hint,
    });
    await response.body?.cancel();
  };
  return {
    async revokeCode(code, clientId) {
      const response = await post("/auth/token", clientId, {
        code,
        grant_type: "authorization_code",
      });
      const tokens = (await response.json().catch(() => null)) as {
        refresh_token?: unknown;
        access_token?: unknown;
      } | null;
      if (typeof tokens?.refresh_token === "string" && tokens.refresh_token)
        return revoke(tokens.refresh_token, "refresh_token", clientId);
      if (typeof tokens?.access_token === "string" && tokens.access_token)
        return revoke(tokens.access_token, "access_token", clientId);
      throw new Error("Apple's answer to /auth/token held no token");
    },
    revokeAccessToken: (token, clientId) =>
      revoke(token, "access_token", clientId),
  };
}

export interface AppleRevocation {
  userId: string;
  /** A Sign in with Apple authorization code the phone sent with the deletion. */
  code: string | undefined;
  /** Which phone app's bundle id the code was issued to, and whether the web Services ID applies. */
  app: PushApp;
}

/**
 * Asks Apple to forget the person's Sign in with Apple, as Apple requires when an account is
 * deleted. With a code from the phone it revokes the token the code trades for; otherwise it
 * revokes the access token Clerk holds from a web sign-in, so it runs before Clerk deletes the
 * user. Whatever Apple or Clerk answer, the deletion goes ahead.
 */
export async function revokeAppleSignIn(
  db: Kysely<Tables>,
  apple: AppleSignIn | null,
  clerk: ClerkBackend | null,
  log: Logger,
  { userId, code, app }: AppleRevocation,
): Promise<void> {
  if (!apple) {
    if (code)
      log.warn(
        { userId },
        "APPLE_SIGNIN_KEY_FILE is not set, so this account's Sign in with Apple stays authorized",
      );
    return;
  }
  const via = code ? "phone" : "clerk";
  try {
    if (code) {
      await apple.revokeCode(code, APNS_TOPICS[app]);
    } else {
      if (!clerk || app !== "production") return;
      const tokens = await clerk.appleAccessTokens(userId);
      if (tokens.length === 0) return;
      for (const token of tokens)
        await apple.revokeAccessToken(token, APPLE_WEB_CLIENT_ID);
    }
  } catch (error) {
    log.warn(
      { userId, via, reason: (error as Error).message },
      "could not revoke Sign in with Apple for a deleted account",
    );
    await audit(db, {
      userId,
      actor: `user:${userId}`,
      action: "account.apple_revoke_failed",
      detail: { via },
    });
    return;
  }
  log.info({ userId, via }, "revoked Sign in with Apple");
  await audit(db, {
    userId,
    actor: `user:${userId}`,
    action: "account.apple_revoked",
    detail: { via },
  });
}
