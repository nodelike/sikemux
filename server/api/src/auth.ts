import {
  createRemoteJWKSet,
  errors,
  jwtVerify,
  type JWTVerifyGetKey,
} from "jose";
import type { Context, MiddlewareHandler } from "hono";
import { sql, type Kysely } from "kysely";

import type { Tables } from "./db.ts";
import { hostRequestMessage, signedBy } from "./devices/signature.ts";
import { ApiFailure, forbidden, unauthorized, type Env } from "./http.ts";

export interface Identity {
  userId: string;
  /**
   * "session" for Clerk session tokens from the phone and the web app, "mac" for the Mac app's
   * OAuth tokens, "host" for a request a host on the account signed with its key.
   */
  via: "session" | "mac" | "host";
  /** The host that signed the request. */
  deviceKey?: string;
  /** When Clerk issued a Mac app's OAuth token. */
  issuedAt?: Date;
  /** The Clerk session a session token belongs to. */
  sessionId?: string;
  /** The browser origin a session token was issued to; native apps have none. */
  origin?: string;
  /** Minutes since the person last proved their first factor, from Clerk's `fva` claim. */
  factorAgeMinutes?: number;
  expiresAt: Date;
}

export interface Verifier {
  verify(token: string): Promise<Identity>;
}

export interface VerifierOptions {
  issuer: string;
  macClientId: string;
  /** Browser origins a session token may come from. Native apps send none. */
  authorizedParties: string[];
  keys?: JWTVerifyGetKey;
}

const OAUTH_TYPES = new Set(["at+jwt", "application/at+jwt"]);

/** Refuses a sign-in to an account that was deleted but can still be restored. */
export function accountDeleted(): ApiFailure {
  return unauthorized(
    "This account was deleted. Sign in to the phone app or app.sikemux.com to restore it.",
  );
}

/**
 * Checks Clerk's tokens against the instance's public keys, without calling Clerk. Session
 * tokens come from the phone and the web app; OAuth access tokens come only from the Mac app.
 */
export function clerkVerifier(options: VerifierOptions): Verifier {
  const keys =
    options.keys ??
    createRemoteJWKSet(new URL("/.well-known/jwks.json", options.issuer));
  return {
    async verify(token) {
      let verified;
      try {
        verified = await jwtVerify(token, keys, {
          issuer: options.issuer,
          algorithms: ["RS256"],
          clockTolerance: 5,
        });
      } catch (error) {
        if (error instanceof errors.JWTExpired)
          throw unauthorized("The sign-in token has expired.");
        if (error instanceof errors.JOSEError)
          throw unauthorized("The sign-in token is not valid.");
        throw error;
      }
      const { payload, protectedHeader } = verified;
      if (typeof payload.sub !== "string" || !payload.sub.startsWith("user_")) {
        throw unauthorized("The sign-in token is not for a user.");
      }
      if (OAUTH_TYPES.has(protectedHeader.typ ?? "")) {
        if (payload.client_id !== options.macClientId) {
          throw unauthorized("The sign-in token was issued to another app.");
        }
        return {
          userId: payload.sub,
          via: "mac",
          issuedAt: new Date((payload.iat ?? 0) * 1000),
          expiresAt: new Date((payload.exp ?? 0) * 1000),
        };
      }
      if (payload.client_id !== undefined)
        throw unauthorized("The sign-in token is an app's, not a session's.");
      if (typeof payload.sid !== "string")
        throw unauthorized("The sign-in token is not a session token.");
      if (payload.sts === "pending")
        throw unauthorized("The sign-in is not finished.");
      if (
        typeof payload.azp === "string" &&
        !options.authorizedParties.includes(payload.azp)
      ) {
        throw unauthorized("The sign-in token was issued to another site.");
      }
      const identity: Identity = {
        userId: payload.sub,
        via: "session",
        sessionId: payload.sid,
        expiresAt: new Date((payload.exp ?? 0) * 1000),
      };
      if (typeof payload.azp === "string") identity.origin = payload.azp;
      const fva: unknown = payload.fva;
      if (Array.isArray(fva) && typeof fva[0] === "number" && fva[0] >= 0)
        identity.factorAgeMinutes = fva[0];
      return identity;
    },
  };
}

/**
 * Refuses a token that is still unexpired but no longer stands: its account was deleted, or it
 * belongs to the session of a phone removed from the account.
 */
export async function checkStanding(
  db: Kysely<Tables>,
  identity: Identity,
  { allowDeleted = false } = {},
): Promise<void> {
  const { rows } = await sql<{ deleted: boolean; removed: boolean }>`
    select
      exists (select 1 from users where id = ${identity.userId} and deleted_at is not null) as deleted,
      exists (
        select 1 from removed_devices
        where clerk_session_id = ${identity.sessionId ?? null}
          and user_id = ${identity.userId}
          and reason = 'removed'
      ) as removed`.execute(db);
  if (rows[0]?.deleted && !allowDeleted) throw accountDeleted();
  if (rows[0]?.removed)
    throw unauthorized("This sign-in was removed from the account.");
}

export type AuthEnv = Env & { Variables: { identity: Identity } };

/**
 * The ways in a route takes. The Mac app's OAuth token is only for adding its host, right after
 * the person signs in in the browser; from then on the host signs its own requests, so removing
 * it from the account cuts it off. "mac" takes that token at any age, for the routes Macs on
 * 0.5.3 and earlier call with it.
 */
export type SignIn = "session" | "host" | "macRegistration" | "mac";

/** How long after its sign-in the Mac app's OAuth token may add its host. */
const MAC_REGISTRATION_MINUTES = 10;
/** How far a signed request's time may be from the server's. */
const HOST_CLOCK_SECONDS = 60;
const HOST_SCHEME =
  /^SikemuxHost\s+([0-9a-f]{64})\.(\d{1,12})\.([0-9a-f]{128})$/;

/** Signatures already taken, with when each stops being accepted, so a request is never made twice. */
const takenSignatures = new Map<string, number>();

function takeSignature(signature: string, until: number) {
  const now = Date.now();
  if (takenSignatures.size > 10_000)
    for (const [taken, expires] of takenSignatures)
      if (expires < now) takenSignatures.delete(taken);
  if (takenSignatures.has(signature))
    throw unauthorized("This signed request was already made.");
  takenSignatures.set(signature, until);
}

async function hostIdentity(
  c: Context,
  db: Kysely<Tables>,
  [key, time, signature]: [string, string, string],
): Promise<Identity> {
  const signedAt = Number(time) * 1000;
  if (Math.abs(Date.now() - signedAt) > HOST_CLOCK_SECONDS * 1000)
    throw unauthorized(
      "The request's time is more than a minute from the server's; check this host's clock.",
    );
  const url = new URL(c.req.url);
  const body = new Uint8Array(await c.req.arrayBuffer());
  const message = hostRequestMessage(
    c.req.method,
    url.pathname + url.search,
    time,
    body,
  );
  if (!signedBy(key, message, signature))
    throw unauthorized("The request is not signed by that host.");
  const host = await db
    .selectFrom("devices")
    .innerJoin("users", "users.id", "devices.user_id")
    .select(["devices.user_id", "users.deleted_at"])
    .where("devices.key", "=", key)
    .where("devices.role", "=", "host")
    .executeTakeFirst();
  if (!host || host.deleted_at)
    throw unauthorized("This host is not on an account.");
  const expiresAt = signedAt + HOST_CLOCK_SECONDS * 1000;
  takeSignature(signature, expiresAt);
  return {
    userId: host.user_id,
    via: "host",
    deviceKey: key,
    expiresAt: new Date(expiresAt),
  };
}

/** Refuses a sign-in that is none of the ways in `accept`. */
function admit(identity: Identity, accept: readonly SignIn[]) {
  const admitted =
    identity.via === "mac"
      ? accept.includes("macRegistration") || accept.includes("mac")
      : accept.includes(identity.via);
  if (!admitted)
    throw forbidden(
      identity.via === "mac"
        ? "The Mac app's sign-in only adds its host to your account."
        : "Only the phone app and app.sikemux.com may do this.",
    );
}

/**
 * Puts who signed in on the context: from `Authorization: Bearer <token>`, or from a request a
 * host signed. Only the ways in `accept` lists get through; by default, the phone's and the web
 * app's sign-ins.
 */
export function requireIdentity(
  verifier: Verifier,
  db: Kysely<Tables>,
  {
    allowDeleted = false,
    accept = ["session"],
  }: { allowDeleted?: boolean; accept?: readonly SignIn[] } = {},
): MiddlewareHandler<AuthEnv> {
  return async (c, next) => {
    const header = c.req.header("authorization") ?? "";
    const host = HOST_SCHEME.exec(header);
    const bearer = /^Bearer\s+(\S+)$/i.exec(header);
    let identity: Identity;
    if (host && accept.includes("host")) {
      identity = await hostIdentity(
        c,
        db,
        host.slice(1) as [string, string, string],
      );
    } else if (bearer?.[1]) {
      identity = await verifier.verify(bearer[1]);
      await checkStanding(db, identity, { allowDeleted });
    } else {
      throw unauthorized(
        accept.includes("host")
          ? "Sign in first: send Authorization: Bearer <token>, or sign the request as a host."
          : "Sign in first: send Authorization: Bearer <token>.",
      );
    }
    admit(identity, accept);
    c.set("identity", identity);
    c.set("log", c.get("log").child({ userId: identity.userId }));
    await next();
  };
}

/** After requireIdentity: narrows a route to some of the ways in its router takes. */
export function allow(...accept: SignIn[]): MiddlewareHandler<AuthEnv> {
  return async (c, next) => {
    const identity = c.get("identity");
    admit(identity, accept);
    if (
      identity.via === "mac" &&
      !accept.includes("mac") &&
      Date.now() - (identity.issuedAt?.getTime() ?? 0) >
        MAC_REGISTRATION_MINUTES * 60_000
    )
      throw unauthorized(
        "The Mac app's sign-in is too old to add a host; sign in again.",
      );
    await next();
  };
}

/** How recently the person must have proved their first factor for what cannot be taken back. */
const REVERIFY_MINUTES = 10;

/** After requireIdentity: asks the phone or the web app to verify the person again first, unless they did lately. */
export function requireRecentSignIn(): MiddlewareHandler<AuthEnv> {
  return async (c, next) => {
    const { factorAgeMinutes } = c.get("identity");
    if (factorAgeMinutes === undefined || factorAgeMinutes > REVERIFY_MINUTES)
      throw new ApiFailure(403, "reverify_required", "reverify", {
        reverifyWithinMinutes: REVERIFY_MINUTES,
      });
    await next();
  };
}

/** Whether the sign-in is the phone's or the web app's. */
export function isSession(identity: Identity): boolean {
  return identity.via === "session";
}

/** Whether the sign-in is the web app's: the verifier lets a session token name only its origin. */
export function isWeb(identity: Identity): boolean {
  return isSession(identity) && identity.origin !== undefined;
}
