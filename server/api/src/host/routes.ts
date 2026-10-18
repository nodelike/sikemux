import { createHash, randomBytes, randomInt } from "node:crypto";
import type {
  Channel,
  HostSignIn,
  HostSignInState,
  PendingHost,
  Platform,
} from "@sikemux/protocol";
import { Hono } from "hono";
import { sql } from "kysely";

import { audit } from "../audit.ts";
import { requireIdentity, type AuthEnv, type Verifier } from "../auth.ts";
import type { Database } from "../db.ts";
import { issueChallenge } from "../devices/challenge.ts";
import { registerDevice, toDevice } from "../devices/register.ts";
import { ApiFailure, readBody } from "../http.ts";
import { clientAddress, limit, type RateLimiter } from "../limits.ts";

const LIFETIME_MS = 10 * 60_000;
const INTERVAL_SECONDS = 5;
/** Consonants only, so no code spells a word or mixes up 0 and O. */
const ALPHABET = "BCDFGHJKLMNPQRSTVWXZ";

function hash(code: string): string {
  return createHash("sha256").update(code, "utf8").digest("hex");
}

function newUserCode(): string {
  const letters = Array.from(
    { length: 8 },
    () => ALPHABET[randomInt(ALPHABET.length)],
  ).join("");
  return `${letters.slice(0, 4)}-${letters.slice(4)}`;
}

/** Accepts what a person types: any case, with or without the dash or spaces. */
function userCodeOf(text: string): string {
  const letters = text.toUpperCase().replace(/[^A-Z]/g, "");
  if (letters.length !== 8 || [...letters].some((l) => !ALPHABET.includes(l)))
    throw gone();
  return `${letters.slice(0, 4)}-${letters.slice(4)}`;
}

function gone(): ApiFailure {
  return new ApiFailure(
    404,
    "not_found",
    "That code is unknown, was refused, or has expired.",
  );
}

/**
 * Signing in a host that has no browser, as in OAuth's device grant: the host shows a user code,
 * the person approves it from a device already signed in, and the host then registers itself
 * with the same signed challenge as any device. The host keeps no sign-in token afterwards.
 */
export function hostSignInRoutes(
  { db }: Database,
  verifier: Verifier,
  limiter: RateLimiter,
  appOrigin: string,
) {
  const perAddress = (name: string, perMinute: number) =>
    limit<AuthEnv>(limiter, name, perMinute, clientAddress);
  const signedIn = requireIdentity(verifier, db);
  const perUser = limit<AuthEnv>(
    limiter,
    "host-sign-in",
    30,
    (c) => c.get("identity").userId,
  );

  const waiting = (code: string) =>
    db
      .selectFrom("host_sign_ins")
      .selectAll()
      .where("code_hash", "=", hash(code))
      .where("expires_at", ">", sql<Date>`now()`)
      .executeTakeFirst();

  return new Hono<AuthEnv>()
    .get("/:userCode", signedIn, perUser, async (c) => {
      const { userId } = c.get("identity");
      const row = await db
        .selectFrom("host_sign_ins")
        .selectAll()
        .where("user_code", "=", userCodeOf(c.req.param("userCode")))
        .where("expires_at", ">", sql<Date>`now()`)
        .where((eb) =>
          eb.or([
            eb("approved_by", "is", null),
            eb("approved_by", "=", userId),
          ]),
        )
        .executeTakeFirst();
      if (!row) throw gone();
      const body: PendingHost = {
        key: row.key,
        name: row.name,
        platform: row.platform as Platform,
        channel: row.channel as Channel,
        expiresAt: row.expires_at.toISOString(),
      };
      return c.json(body);
    })
    .post("/:userCode/approve", signedIn, perUser, async (c) => {
      const { userId, via } = c.get("identity");
      const row = await db
        .updateTable("host_sign_ins")
        .set({ approved_by: userId })
        .where("user_code", "=", userCodeOf(c.req.param("userCode")))
        .where("expires_at", ">", sql<Date>`now()`)
        .where((eb) =>
          eb.or([
            eb("approved_by", "is", null),
            eb("approved_by", "=", userId),
          ]),
        )
        .returning(["key", "name"])
        .executeTakeFirst();
      if (!row) throw gone();
      await audit(db, {
        userId,
        actor: `user:${userId}`,
        action: "host_sign_in.approved",
        subject: row.key,
        detail: { name: row.name, via },
      });
      c.get("log").info({ host: row.key.slice(0, 8) }, "approved a host");
      return c.body(null, 204);
    })
    .delete("/:userCode", signedIn, perUser, async (c) => {
      const { userId, via } = c.get("identity");
      const row = await db
        .deleteFrom("host_sign_ins")
        .where("user_code", "=", userCodeOf(c.req.param("userCode")))
        .where("expires_at", ">", sql<Date>`now()`)
        .where((eb) =>
          eb.or([
            eb("approved_by", "is", null),
            eb("approved_by", "=", userId),
          ]),
        )
        .returning(["key", "name"])
        .executeTakeFirst();
      if (!row) throw gone();
      await audit(db, {
        userId,
        actor: `user:${userId}`,
        action: "host_sign_in.refused",
        subject: row.key,
        detail: { name: row.name, via },
      });
      return c.body(null, 204);
    })
    .post("/", perAddress("host-sign-in-start", 10), async (c) => {
      const start = await readBody(c, "HostSignInStart");
      const name = start.name.trim();
      if (!name)
        throw new ApiFailure(400, "bad_request", "The host needs a name.");
      await db
        .deleteFrom("host_sign_ins")
        .where("expires_at", "<", sql<Date>`now()`)
        .execute();
      const code = randomBytes(32).toString("hex");
      const expiresAt = new Date(Date.now() + LIFETIME_MS);
      for (;;) {
        const userCode = newUserCode();
        const inserted = await db
          .insertInto("host_sign_ins")
          .values({
            code_hash: hash(code),
            user_code: userCode,
            key: start.key,
            name,
            platform: start.platform,
            channel: start.channel,
            expires_at: expiresAt,
          })
          .onConflict((oc) => oc.column("user_code").doNothing())
          .returning("user_code")
          .executeTakeFirst();
        if (!inserted) continue;
        const body: HostSignIn = {
          code,
          userCode,
          verificationUrl: `${appOrigin}/host?code=${userCode}`,
          expiresAt: expiresAt.toISOString(),
          interval: INTERVAL_SECONDS,
        };
        return c.json(body, 201);
      }
    })
    .post("/poll", perAddress("host-sign-in-poll", 60), async (c) => {
      const { code } = await readBody(c, "HostSignInPoll");
      const row = await waiting(code);
      if (!row) throw gone();
      if (!row.approved_by)
        return c.json<HostSignInState>({ status: "pending" });
      const { nonce } = await issueChallenge(db, row.approved_by);
      return c.json<HostSignInState>({
        status: "approved",
        account: row.approved_by,
        nonce,
      });
    })
    .post("/finish", perAddress("host-sign-in-finish", 10), async (c) => {
      const finish = await readBody(c, "HostSignInFinish");
      const row = await waiting(finish.code);
      if (!row?.approved_by) throw gone();
      const { row: device, added } = await registerDevice(
        db,
        { userId: row.approved_by, via: "code" },
        {
          key: row.key,
          role: "host",
          name: row.name,
          platform: row.platform as Platform,
          channel: row.channel as Channel,
          nonce: finish.nonce,
          signature: finish.signature,
        },
      );
      await db
        .deleteFrom("host_sign_ins")
        .where("code_hash", "=", row.code_hash)
        .execute();
      c.get("log").info({ key: device.key, added }, "signed in a host by code");
      return c.json(toDevice(device), added ? 201 : 200);
    });
}
