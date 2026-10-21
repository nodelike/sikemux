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
import {
  requireIdentity,
  requireRecentSignIn,
  type AuthEnv,
  type Verifier,
} from "../auth.ts";
import type { Database } from "../db.ts";
import { issueChallenge } from "../devices/challenge.ts";
import { registerDevice, toDevice } from "../devices/register.ts";
import type { ApiFailure } from "../http.ts";
import { badRequest, notFound, readBody } from "../http.ts";
import type { Logger } from "../log.ts";
import type { Places } from "../places.ts";
import {
  callerAddress,
  clientAddress,
  limit,
  type RateLimiter,
} from "../limits.ts";

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
  return notFound("That code is unknown, was refused, or has expired.");
}

/**
 * Signing in a host that has no browser, as in OAuth's device grant: the host shows a user code,
 * the person types it on their phone or the web app and approves it, and the host then registers
 * itself with the same signed challenge as any device. The host keeps no sign-in token afterwards.
 */
export function hostSignInRoutes(
  { db }: Database,
  verifier: Verifier,
  limiter: RateLimiter,
  appOrigin: string,
  places: Places,
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

  const fromHost = requireIdentity(verifier, db, { accept: ["host"] });

  const approve = async (
    userId: string,
    via: string,
    userCode: string,
    log: Logger,
  ) => {
    const row = await db
      .updateTable("host_sign_ins")
      .set({ approved_by: userId })
      .where("user_code", "=", userCodeOf(userCode))
      .where("expires_at", ">", sql<Date>`now()`)
      .where((eb) =>
        eb.or([eb("approved_by", "is", null), eb("approved_by", "=", userId)]),
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
    log.info({ host: row.key.slice(0, 8) }, "approved a host");
    return row;
  };

  const waiting = (code: string) =>
    db
      .selectFrom("host_sign_ins")
      .selectAll()
      .where("code_hash", "=", hash(code))
      .where("expires_at", ">", sql<Date>`now()`)
      .executeTakeFirst();

  return (
    new Hono<AuthEnv>()
      .get("/:userCode", signedIn, perUser, async (c) => {
        const { userId } = c.get("identity");
        const row = await db
          .selectFrom("host_sign_ins")
          .leftJoin("removed_devices", (join) =>
            join
              .onRef("removed_devices.key", "=", "host_sign_ins.key")
              .on("removed_devices.user_id", "=", userId)
              .on("removed_devices.reason", "=", "removed"),
          )
          .selectAll("host_sign_ins")
          .select("removed_devices.removed_at")
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
          requestedAt: row.created_at.toISOString(),
          requestedFrom: row.requested_from,
          requestedNear: row.requested_from
            ? places.near(row.requested_from)
            : null,
        };
        if (row.removed_at) body.removedAt = row.removed_at.toISOString();
        return c.json(body);
      })
      .post(
        "/:userCode/approve",
        signedIn,
        requireRecentSignIn(),
        perUser,
        async (c) => {
          const { userId, via } = c.get("identity");
          await approve(userId, via, c.req.param("userCode"), c.get("log"));
          return c.body(null, 204);
        },
      )
      // "Install Sikemux here" in the desktop app: it runs the installer on a server over the
      // person's own SSH, reads the code `sikemux login` prints there, and has its host approve that
      // code. A host signs only for the person in its own app, never for an agent or any other
      // program, so this trusts the host's signature as the person's click (ADR 0018).
      .post("/:userCode/approve-from-host", fromHost, perUser, async (c) => {
        const { userId, via } = c.get("identity");
        return c.json(
          await approve(userId, via, c.req.param("userCode"), c.get("log")),
        );
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
        if (!name) throw badRequest("The host needs a name.");
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
              requested_from: callerAddress(c)?.slice(0, 64) ?? null,
              expires_at: expiresAt,
            })
            .onConflict((oc) => oc.column("user_code").doNothing())
            .returning("user_code")
            .executeTakeFirst();
          if (!inserted) continue;
          const body: HostSignIn = {
            code,
            userCode,
            verificationUrl: `${appOrigin}/host`,
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
        c.get("log").info(
          { key: device.key, added },
          "signed in a host by code",
        );
        return c.json(toDevice(device), added ? 201 : 200);
      })
  );
}
