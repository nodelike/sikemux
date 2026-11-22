import { randomBytes } from "node:crypto";
import type { Challenge } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { badRequest, forbidden } from "../http.ts";
import { signedBy } from "./signature.ts";

const CHALLENGE_MS = 2 * 60_000;

/**
 * A one-time value for the account's device to sign, good for two minutes. Given when a Mac's
 * browser sign-in was made, it is the only challenge that adds that Mac with no other sign-in.
 */
export async function issueChallenge(
  db: Kysely<Tables>,
  userId: string,
  { macSignedInAt }: { macSignedInAt?: Date } = {},
): Promise<Challenge> {
  const nonce = randomBytes(32).toString("hex");
  const expiresAt = new Date(Date.now() + CHALLENGE_MS);
  await pruneChallenges(db);
  await db
    .insertInto("challenges")
    .values({
      nonce,
      user_id: userId,
      expires_at: expiresAt,
      mac_signed_in_at: macSignedInAt ?? null,
    })
    .execute();
  return { nonce, expiresAt: expiresAt.toISOString() };
}

/** Who a Mac's unused, unexpired sign-in challenge is for, and when that sign-in was made. */
export async function macSignInChallenge(
  db: Kysely<Tables>,
  nonce: string,
): Promise<{ userId: string; signedInAt: Date }> {
  const challenge = await db
    .selectFrom("challenges")
    .select(["user_id", "mac_signed_in_at"])
    .where("nonce", "=", nonce)
    .where("mac_signed_in_at", "is not", null)
    .where("expires_at", ">", sql<Date>`now()`)
    .executeTakeFirst();
  if (!challenge?.mac_signed_in_at)
    throw badRequest("The challenge is unknown, already used, or expired.");
  return { userId: challenge.user_id, signedInAt: challenge.mac_signed_in_at };
}

/**
 * Uses up the account's challenge, refusing one that is unknown, already used or expired. A Mac's
 * sign-in challenge is taken only where `macSignIn` asks for one, and nowhere else.
 */
async function consumeChallenge(
  db: Kysely<Tables>,
  userId: string,
  nonce: string,
  macSignIn: boolean,
): Promise<void> {
  const challenge = await db
    .deleteFrom("challenges")
    .where("nonce", "=", nonce)
    .where("user_id", "=", userId)
    .where("mac_signed_in_at", macSignIn ? "is not" : "is", null)
    .where("expires_at", ">", sql<Date>`now()`)
    .returning("nonce")
    .executeTakeFirst();
  if (!challenge)
    throw badRequest("The challenge is unknown, already used, or expired.");
}

/** Uses up the account's challenge and checks the device signed `message` with its key. */
export async function proveKey(
  db: Kysely<Tables>,
  userId: string,
  proof: { nonce: string; key: string; message: string; signature: string },
  { macSignIn = false } = {},
): Promise<void> {
  await consumeChallenge(db, userId, proof.nonce, macSignIn);
  if (!signedBy(proof.key, proof.message, proof.signature))
    throw forbidden("The signature does not prove this device holds the key.");
}

/** Deletes challenges that expired over an hour ago, returning how many. */
export async function pruneChallenges(db: Kysely<Tables>): Promise<number> {
  const { numDeletedRows } = await db
    .deleteFrom("challenges")
    .where("expires_at", "<", sql<Date>`now() - interval '1 hour'`)
    .executeTakeFirst();
  return Number(numDeletedRows);
}
