import { createPrivateKey, type KeyObject } from "node:crypto";
import { readFileSync } from "node:fs";

import { signJwt } from "../jwt.ts";
import { refused, unanswered, worthRetrying } from "./delivery.ts";
import type { Delivery, PushMessage, PushProvider } from "./provider.ts";

/** The parts of a Google service account's JSON key that sending to FCM needs. */
export interface ServiceAccount {
  projectId: string;
  clientEmail: string;
  privateKey: KeyObject;
  privateKeyId: string | null;
  tokenUri: string;
}

const SCOPE = "https://www.googleapis.com/auth/firebase.messaging";
const DEFAULT_TOKEN_URI = "https://oauth2.googleapis.com/token";
const ASSERTION_SECONDS = 3600;
/** An access token is replaced this long before Google says it expires. */
const REFRESH_EARLY_MS = 5 * 60_000;
const REQUEST_TIMEOUT_MS = 10_000;
/** FCM keeps an undelivered message at most four weeks. */
const MAX_TTL_SECONDS = 28 * 24 * 3600;

/** Reads a service account key file, throwing a message that names what is wrong with it. */
export function readServiceAccount(path: string): ServiceAccount {
  let text: string;
  try {
    text = readFileSync(path, "utf8");
  } catch (error) {
    throw new Error(
      `cannot be read (${(error as NodeJS.ErrnoException).code ?? "unknown error"})`,
      {
        cause: error,
      },
    );
  }
  let json: Record<string, unknown>;
  try {
    json = JSON.parse(text) as Record<string, unknown>;
  } catch {
    throw new Error("is not JSON");
  }
  const field = (name: string) => {
    const value = json[name];
    return typeof value === "string" && value.trim() ? value : null;
  };
  const projectId = field("project_id");
  const clientEmail = field("client_email");
  const pem = field("private_key");
  if (json.type !== "service_account" || !projectId || !clientEmail || !pem)
    throw new Error(
      "is not a service account key with project_id, client_email and private_key",
    );
  let privateKey: KeyObject;
  try {
    privateKey = createPrivateKey(pem);
  } catch {
    throw new Error("has a private_key that is not a PEM private key");
  }
  const tokenUri = field("token_uri") ?? DEFAULT_TOKEN_URI;
  if (!URL.canParse(tokenUri))
    throw new Error("has a token_uri that is not a URL");
  return {
    projectId,
    clientEmail,
    privateKey,
    privateKeyId: field("private_key_id"),
    tokenUri,
  };
}

/** The signed assertion Google trades for an access token (RFC 7523). */
export function signAssertion(account: ServiceAccount, nowMs: number): string {
  const iat = Math.floor(nowMs / 1000);
  return signJwt(
    {
      alg: "RS256",
      typ: "JWT",
      ...(account.privateKeyId ? { kid: account.privateKeyId } : {}),
    },
    {
      iss: account.clientEmail,
      scope: SCOPE,
      aud: account.tokenUri,
      iat,
      exp: iat + ASSERTION_SECONDS,
    },
    account.privateKey,
  );
}

interface FcmError {
  status?: string;
  message?: string;
  details?: { "@type"?: string; errorCode?: string }[];
}

async function readError(response: Response): Promise<FcmError> {
  try {
    const body = (await response.json()) as { error?: FcmError };
    return body.error ?? {};
  } catch {
    return {};
  }
}

/** The error code FCM puts in its details, falling back to the HTTP status's name. */
function errorCode(error: FcmError): string {
  const detail = error.details?.find((item) => item.errorCode)?.errorCode;
  return detail ?? error.status ?? "UNKNOWN";
}

class TokenRefused extends Error {
  readonly retryable: boolean;

  constructor(message: string, retryable: boolean) {
    super(message);
    this.retryable = retryable;
  }
}

export interface FcmOptions {
  /** Where FCM's API is; tests point it at a fake. */
  endpoint?: string;
  now?: () => number;
}

/** Sends data-only messages through FCM's HTTP v1 API, as the service account. */
export class FcmProvider implements PushProvider {
  private readonly account: ServiceAccount;
  private readonly endpoint: string;
  private readonly now: () => number;
  private accessToken: { value: string; refreshAt: number } | null = null;
  private minting: Promise<string> | null = null;

  constructor(account: ServiceAccount, options: FcmOptions = {}) {
    this.account = account;
    this.endpoint = options.endpoint ?? "https://fcm.googleapis.com";
    this.now = options.now ?? Date.now;
  }

  async send(message: PushMessage): Promise<Delivery> {
    let token: string;
    try {
      token = await this.token();
    } catch (error) {
      return this.mintFailed(error);
    }
    let response = await this.post(message, token);
    if (response instanceof Error) return unanswered(response);
    if (response.status === 401) {
      this.accessToken = null;
      try {
        token = await this.token();
      } catch (error) {
        return this.mintFailed(error);
      }
      response = await this.post(message, token);
      if (response instanceof Error) return unanswered(response);
    }
    if (response.ok) {
      await response.body?.cancel();
      return { outcome: "sent" };
    }
    const code = errorCode(await readError(response));
    if (response.status === 404 || code === "UNREGISTERED")
      return { outcome: "dead", reason: code };
    if (code === "INVALID_ARGUMENT" || code === "SENDER_ID_MISMATCH")
      return { outcome: "dead", reason: code };
    return refused(response.status, code, response.headers.get("retry-after"));
  }

  private mintFailed(error: unknown): Delivery {
    const reason = error instanceof Error ? error.message : String(error);
    return error instanceof TokenRefused && !error.retryable
      ? { outcome: "failed", reason }
      : { outcome: "retry", reason };
  }

  private async post(
    message: PushMessage,
    token: string,
  ): Promise<Response | Error> {
    const body = {
      message: {
        token: message.token,
        data: { b: message.blob, c: message.collapseId, t: message.kind },
        android: {
          priority: message.kind === "alert" ? "high" : "normal",
          ttl: `${Math.min(MAX_TTL_SECONDS, Math.max(0, Math.ceil(message.ttlSeconds)))}s`,
        },
      },
    };
    try {
      return await fetch(
        `${this.endpoint}/v1/projects/${encodeURIComponent(this.account.projectId)}/messages:send`,
        {
          method: "POST",
          headers: {
            authorization: `Bearer ${token}`,
            "content-type": "application/json",
          },
          body: JSON.stringify(body),
          signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
        },
      );
    } catch (error) {
      return error instanceof Error ? error : new Error(String(error));
    }
  }

  /** A cached access token, minting one when there is none or it is about to expire. */
  private token(): Promise<string> {
    if (this.accessToken && this.now() < this.accessToken.refreshAt)
      return Promise.resolve(this.accessToken.value);
    this.minting ??= this.mint().finally(() => {
      this.minting = null;
    });
    return this.minting;
  }

  private async mint(): Promise<string> {
    const issued = this.now();
    let response: Response;
    try {
      response = await fetch(this.account.tokenUri, {
        method: "POST",
        headers: { "content-type": "application/x-www-form-urlencoded" },
        body: new URLSearchParams({
          grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
          assertion: signAssertion(this.account, issued),
        }),
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      });
    } catch (error) {
      throw new TokenRefused(
        `minting an access token failed: ${error instanceof Error ? error.message : String(error)}`,
        true,
      );
    }
    const body = (await response.json().catch(() => ({}))) as {
      access_token?: unknown;
      expires_in?: unknown;
      error?: unknown;
    };
    if (!response.ok || typeof body.access_token !== "string")
      throw new TokenRefused(
        `Google refused an access token: ${response.status} ${typeof body.error === "string" ? body.error : ""}`.trim(),
        worthRetrying(response.status),
      );
    const lifetimeMs =
      (typeof body.expires_in === "number" ? body.expires_in : 3600) * 1000;
    this.accessToken = {
      value: body.access_token,
      refreshAt: issued + Math.max(0, lifetimeMs - REFRESH_EARLY_MS),
    };
    return body.access_token;
  }
}
