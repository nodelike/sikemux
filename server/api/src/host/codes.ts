import { createHash, randomBytes, randomInt } from "node:crypto";
import type {
  HostSignIn,
  HostSignInFinish,
  HostSignInStart,
  HostSignInState,
  HostTokenGrant,
  PendingHost,
} from "@sikemux/protocol";
import { sql, type ExpressionBuilder, type Kysely } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { issueChallenge } from "../devices/challenge.ts";
import { registerDevice } from "../devices/register.ts";
import { badRequest, notFound, type ApiFailure } from "../http.ts";
import type { Places } from "../places.ts";

const LIFETIME_MS = 10 * 60_000;
const TOKEN_LIFETIME_MS = 60 * 60_000;
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

/** The sign-in `userCode` names, while it lasts and nobody but `userId` has approved it. */
function openTo(userId: string, userCode: string) {
  return (eb: ExpressionBuilder<Tables, "host_sign_ins">) =>
    eb.and([
      eb("user_code", "=", userCodeOf(userCode)),
      eb("expires_at", ">", sql<Date>`now()`),
      eb.or([eb("approved_by", "is", null), eb("approved_by", "=", userId)]),
    ]);
}

function waiting(db: Kysely<Tables>, code: string) {
  return db
    .selectFrom("host_sign_ins")
    .selectAll()
    .where("code_hash", "=", hash(code))
    .where("expires_at", ">", sql<Date>`now()`)
    .executeTakeFirst();
}

/** The host waiting behind `userCode`, for the person to check before approving it. */
export async function pendingHost(
  db: Kysely<Tables>,
  userId: string,
  userCode: string,
  places: Places,
): Promise<PendingHost> {
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
    .where(openTo(userId, userCode))
    .executeTakeFirst();
  if (!row) throw gone();
  const host: PendingHost = {
    key: row.key,
    name: row.name,
    platform: row.platform,
    channel: row.channel,
    expiresAt: row.expires_at.toISOString(),
    requestedAt: row.created_at.toISOString(),
    requestedFrom: row.requested_from,
    requestedNear: row.requested_from ? places.near(row.requested_from) : null,
  };
  if (row.removed_at) host.removedAt = row.removed_at.toISOString();
  return host;
}

/** Lets the host behind `userCode` onto `userId`'s account, answering which host it is. */
export async function approveHost(
  db: Kysely<Tables>,
  { userId, via }: { userId: string; via: string },
  userCode: string,
) {
  const row = await db
    .updateTable("host_sign_ins")
    .set({ approved_by: userId })
    .where(openTo(userId, userCode))
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
  return row;
}

export async function refuseHost(
  db: Kysely<Tables>,
  { userId, via }: { userId: string; via: string },
  userCode: string,
) {
  const row = await db
    .deleteFrom("host_sign_ins")
    .where(openTo(userId, userCode))
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
}

/** A token that lets one host onto `userId`'s account, with the command a server runs to use it. */
export async function createHostToken(
  db: Kysely<Tables>,
  { userId, via }: { userId: string; via: string },
): Promise<HostTokenGrant> {
  await db
    .deleteFrom("host_tokens")
    .where("expires_at", "<", sql<Date>`now()`)
    .execute();
  const token = randomBytes(32).toString("hex");
  const expiresAt = new Date(Date.now() + TOKEN_LIFETIME_MS);
  await db
    .insertInto("host_tokens")
    .values({ token_hash: hash(token), user_id: userId, expires_at: expiresAt })
    .execute();
  await audit(db, {
    userId,
    actor: `user:${userId}`,
    action: "host_token.created",
    subject: hash(token).slice(0, 16),
    detail: { via },
  });
  return {
    token,
    command: `curl -fsSL https://sikemux.com/install | sh -s -- --host --yes --token ${token}`,
    expiresAt: expiresAt.toISOString(),
  };
}

/** Uses up `token`, answering the account that made it. */
async function spendToken(db: Kysely<Tables>, token: string, key: string) {
  const row = await db
    .deleteFrom("host_tokens")
    .where("token_hash", "=", hash(token))
    .where("expires_at", ">", sql<Date>`now()`)
    .returning("user_id")
    .executeTakeFirst();
  if (!row)
    throw notFound(
      "That token is unknown, used or expired. Make a new one in Sikemux.",
    );
  await audit(db, {
    userId: row.user_id,
    actor: `device:${key}`,
    action: "host_token.used",
    subject: hash(token).slice(0, 16),
  });
  return row.user_id;
}

/**
 * Starts a host's sign-in from `requestedFrom`, with a user code no other sign-in holds. A sign-in
 * with a token is approved at once by the account that made it.
 */
export async function startSignIn(
  db: Kysely<Tables>,
  start: HostSignInStart,
  requestedFrom: string | null,
  appOrigin: string,
): Promise<HostSignIn> {
  const name = start.name.trim();
  if (!name) throw badRequest("The host needs a name.");
  await db
    .deleteFrom("host_sign_ins")
    .where("expires_at", "<", sql<Date>`now()`)
    .execute();
  const code = randomBytes(32).toString("hex");
  const expiresAt = new Date(Date.now() + LIFETIME_MS);
  return db.transaction().execute(async (trx) => {
    const approvedBy = start.token
      ? await spendToken(trx, start.token, start.key)
      : null;
    for (;;) {
      const userCode = newUserCode();
      const inserted = await trx
        .insertInto("host_sign_ins")
        .values({
          code_hash: hash(code),
          user_code: userCode,
          key: start.key,
          name,
          platform: start.platform,
          channel: start.channel,
          approved_by: approvedBy,
          requested_from: requestedFrom,
          expires_at: expiresAt,
        })
        .onConflict((oc) => oc.column("user_code").doNothing())
        .returning("user_code")
        .executeTakeFirst();
      if (!inserted) continue;
      return {
        code,
        userCode,
        verificationUrl: `${appOrigin}/host`,
        expiresAt: expiresAt.toISOString(),
        interval: INTERVAL_SECONDS,
      };
    }
  });
}
/** Whether the person has approved the sign-in yet, with the challenge the host then signs. */
export async function pollSignIn(
  db: Kysely<Tables>,
  code: string,
): Promise<HostSignInState> {
  const row = await waiting(db, code);
  if (!row) throw gone();
  if (!row.approved_by) return { status: "pending" };
  const { nonce } = await issueChallenge(db, row.approved_by);
  return { status: "approved", account: row.approved_by, nonce };
}

/** Registers the approved host with its signed challenge, ending its sign-in. */
export async function finishSignIn(
  db: Kysely<Tables>,
  finish: HostSignInFinish,
) {
  const row = await waiting(db, finish.code);
  if (!row?.approved_by) throw gone();
  const registered = await registerDevice(
    db,
    { userId: row.approved_by, via: "code" },
    {
      key: row.key,
      role: "host",
      name: row.name,
      platform: row.platform,
      channel: row.channel,
      nonce: finish.nonce,
      signature: finish.signature,
    },
  );
  await db
    .deleteFrom("host_sign_ins")
    .where("code_hash", "=", row.code_hash)
    .execute();
  return registered;
}
