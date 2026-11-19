import { createHash, randomBytes } from "node:crypto";
import { createRemoteJWKSet, jwtVerify } from "jose";

import { ApiFailure, conflict } from "../http.ts";

const GOOGLE = {
  issuers: ["https://accounts.google.com", "accounts.google.com"],
  authorize: "https://accounts.google.com/o/oauth2/v2/auth",
  token: "https://oauth2.googleapis.com/token",
  keys: "https://www.googleapis.com/oauth2/v3/certs",
};
const TIMEOUT_MS = 10_000;

export interface WorkGoogleSettings {
  clientId: string;
  clientSecret: string;
  issuers: string[];
  authorize: string;
  token: string;
  keys: string;
}

/**
 * Reads GOOGLE_WORK_ADDRESS_CLIENT_ID and GOOGLE_WORK_ADDRESS_CLIENT_SECRET. On a local run
 * against the stand-in sign-in, Google is the stand-in's too.
 */
export function readWorkGoogle(
  env: NodeJS.ProcessEnv,
  problems: string[],
  standIn: string | null,
): WorkGoogleSettings | null {
  const clientId = env.GOOGLE_WORK_ADDRESS_CLIENT_ID?.trim() || null;
  const clientSecret = env.GOOGLE_WORK_ADDRESS_CLIENT_SECRET?.trim() || null;
  if (!clientId && !clientSecret) return null;
  if (!clientId || !clientSecret) {
    problems.push(
      "GOOGLE_WORK_ADDRESS_CLIENT_ID and GOOGLE_WORK_ADDRESS_CLIENT_SECRET are set together or not at all",
    );
    return null;
  }
  if (!standIn && !clientId.endsWith(".apps.googleusercontent.com"))
    problems.push(
      "GOOGLE_WORK_ADDRESS_CLIENT_ID is not a Google client id like ….apps.googleusercontent.com",
    );
  const endpoints = standIn
    ? {
        issuers: [`${standIn}/google`],
        authorize: `${standIn}/google/authorize`,
        token: `${standIn}/google/token`,
        keys: `${standIn}/google/keys`,
      }
    : GOOGLE;
  return { clientId, clientSecret, ...endpoints };
}

export interface WorkGoogleStart {
  url: string;
  state: string;
  nonce: string;
  verifier: string;
}

/** Signing in to Google only to learn which verified address the person holds there. */
export interface WorkGoogle {
  start(redirectUri: string): WorkGoogleStart;
  /** The verified address Google signed in; throws when it signed in none. Never log it. */
  address(asked: {
    code: string;
    verifier: string;
    nonce: string;
    redirectUri: string;
  }): Promise<string>;
}

const token = () => randomBytes(32).toString("base64url");

export function workGoogle(
  settings: WorkGoogleSettings,
  fetcher: typeof fetch = fetch,
): WorkGoogle {
  const keys = createRemoteJWKSet(new URL(settings.keys));
  return {
    start(redirectUri) {
      const state = token();
      const nonce = token();
      const verifier = token();
      const url = new URL(settings.authorize);
      url.search = new URLSearchParams({
        client_id: settings.clientId,
        redirect_uri: redirectUri,
        response_type: "code",
        scope: "openid email",
        state,
        nonce,
        code_challenge: createHash("sha256")
          .update(verifier)
          .digest("base64url"),
        code_challenge_method: "S256",
        prompt: "select_account",
      }).toString();
      return { url: url.toString(), state, nonce, verifier };
    },
    async address({ code, verifier, nonce, redirectUri }) {
      let response: Response;
      try {
        response = await fetcher(settings.token, {
          method: "POST",
          headers: { "content-type": "application/x-www-form-urlencoded" },
          body: new URLSearchParams({
            grant_type: "authorization_code",
            code,
            code_verifier: verifier,
            client_id: settings.clientId,
            client_secret: settings.clientSecret,
            redirect_uri: redirectUri,
          }),
          signal: AbortSignal.timeout(TIMEOUT_MS),
        });
      } catch {
        throw new ApiFailure(
          503,
          "unavailable",
          "Google can't be reached right now. Try again in a minute.",
        );
      }
      const body = (await response.json().catch(() => ({}))) as {
        id_token?: unknown;
      };
      if (!response.ok || typeof body.id_token !== "string")
        throw conflict(
          "Google didn't confirm the sign-in. Start again from Work addresses.",
        );
      const { payload } = await jwtVerify(body.id_token, keys, {
        issuer: settings.issuers,
        audience: settings.clientId,
        algorithms: ["RS256"],
      }).catch(() => {
        throw conflict(
          "Google's answer couldn't be checked. Start again from Work addresses.",
        );
      });
      if (payload.nonce !== nonce)
        throw conflict(
          "Google's answer was for another try. Start again from Work addresses.",
        );
      if (payload.email_verified !== true || typeof payload.email !== "string")
        throw conflict(
          "Google has no verified address on that account, so nothing was proven.",
        );
      return payload.email;
    },
  };
}
