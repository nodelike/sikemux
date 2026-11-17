import {
  createRemoteJWKSet,
  errors,
  jwtVerify,
  type JWTVerifyGetKey,
} from "jose";
import type { Context, MiddlewareHandler } from "hono";
import { sql, type Kysely } from "kysely";

import type { Tables } from "./db.ts";
import { bodySha256, requestMessage, signedBy } from "./devices/signature.ts";
import {
  ApiFailure,
  badRequest,
  forbidden,
  unauthorized,
  type Env,
} from "./http.ts";
import { standingDevices } from "./devices/standing.ts";
import { reportAs } from "./sentry.ts";

export interface Identity {
  userId: string;
  /**
   * "session" for Clerk session tokens from the phone and the web app, "mac" for the Mac app's
   * OAuth tokens, "host" for a request a host on the account signed with its key, "device" for
   * one a phone on the account signed with its key.
   */
  via: "session" | "mac" | "host" | "device";
  /** The host or phone that signed the request. */
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

interface VerifierOptions {
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

export type AuthEnv = Env & {
  Variables: {
    identity: Identity;
    /**
     * Holds a streamed body, by its SHA-256 in hex, to the hash a signed request declared. Does
     * nothing for a bearer token.
     */
    checkBody: (bodySha256: string) => void;
  };
};

/**
 * The ways in a route takes. The Mac app's OAuth token is only for adding its host, right after
 * the person signs in in the browser; from then on the host signs its own requests, so removing
 * it from the account cuts it off. "mac" takes that token at any age, for the routes Macs on
 * 0.5.3 and earlier call with it.
 */
export type SignIn = "session" | "host" | "device" | "macRegistration" | "mac";

/** How long after its sign-in the Mac app's OAuth token may add its host. */
const MAC_REGISTRATION_MINUTES = 10;
/** How far a signed request's time may be from the server's. */
const HOST_CLOCK_SECONDS = 60;
const SIGNED_SCHEME =
  /^Sikemux(Host|Device)\s+([0-9a-f]{64})\.(\d{1,12})\.([0-9a-f]{128})$/;

const SIGNERS = {
  Host: { via: "host", role: "host" },
  Device: { via: "device", role: "client" },
} as const;

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

/**
 * Who signed a request with a host's or a phone's key. A streamed body is not read here: the
 * signature covers the SHA-256 the request declares in X-Body-Sha256, and checkBody holds the
 * body to it once the handler has hashed it.
 */
async function signedIdentity(
  c: Context,
  db: Kysely<Tables>,
  [scheme, key, time, signature]: [
    keyof typeof SIGNERS,
    string,
    string,
    string,
  ],
  streamed: boolean,
): Promise<{ identity: Identity; checkBody: (bodySha256: string) => void }> {
  const signer = SIGNERS[scheme];
  const signedAt = Number(time) * 1000;
  if (Math.abs(Date.now() - signedAt) > HOST_CLOCK_SECONDS * 1000)
    throw unauthorized(
      `The request's time is more than a minute from the server's; check this ${signer.via}'s clock.`,
    );
  const declared = streamed ? c.req.header("x-body-sha256") : undefined;
  if (streamed && !/^[0-9a-f]{64}$/.test(declared ?? ""))
    throw badRequest(
      "A signed upload declares its body's SHA-256, in lowercase hex, in X-Body-Sha256.",
    );
  const url = new URL(c.req.url);
  const message = requestMessage(
    signer.via,
    c.req.method,
    url.pathname + url.search,
    time,
    declared ?? bodySha256(new Uint8Array(await c.req.arrayBuffer())),
  );
  if (!signedBy(key, message, signature))
    throw unauthorized(`The request is not signed by that ${signer.via}.`);
  const device = await standingDevices(db)
    .select("devices.user_id")
    .where("devices.key", "=", key)
    .where("devices.role", "=", signer.role)
    .executeTakeFirst();
  if (!device) throw unauthorized(`This ${signer.via} is not on an account.`);
  const expiresAt = signedAt + HOST_CLOCK_SECONDS * 1000;
  takeSignature(signature, expiresAt);
  return {
    identity: {
      userId: device.user_id,
      via: signer.via,
      deviceKey: key,
      expiresAt: new Date(expiresAt),
    },
    checkBody: (sha256) => {
      if (declared !== undefined && sha256 !== declared)
        throw badRequest("The body does not match its X-Body-Sha256.");
    },
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
    streamed = () => false,
  }: {
    allowDeleted?: boolean;
    accept?: readonly SignIn[];
    /** Whether a request's body is left for its handler to hash as it streams it. */
    streamed?: (c: Context) => boolean;
  } = {},
): MiddlewareHandler<AuthEnv> {
  return async (c, next) => {
    const header = c.req.header("authorization") ?? "";
    const signed = SIGNED_SCHEME.exec(header);
    const bearer = /^Bearer\s+(\S+)$/i.exec(header);
    let identity: Identity;
    let checkBody: (bodySha256: string) => void = () => undefined;
    const scheme = signed?.[1] as keyof typeof SIGNERS | undefined;
    if (signed && scheme && accept.includes(SIGNERS[scheme].via)) {
      ({ identity, checkBody } = await signedIdentity(
        c,
        db,
        signed.slice(1) as [keyof typeof SIGNERS, string, string, string],
        streamed(c),
      ));
    } else if (bearer?.[1]) {
      identity = await verifier.verify(bearer[1]);
      await checkStanding(db, identity, { allowDeleted });
    } else {
      throw unauthorized(
        accept.includes("device")
          ? "Sign in first: send Authorization: Bearer <token>, or sign the request with the device's key."
          : accept.includes("host")
            ? "Sign in first: send Authorization: Bearer <token>, or sign the request as a host."
            : "Sign in first: send Authorization: Bearer <token>.",
      );
    }
    admit(identity, accept);
    c.set("identity", identity);
    c.set("checkBody", checkBody);
    c.set("log", c.get("log").child({ userId: identity.userId }));
    reportAs(identity.userId);
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
