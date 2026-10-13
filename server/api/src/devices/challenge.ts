import { randomBytes } from "node:crypto";
import type { Challenge } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { ApiFailure } from "../http.ts";

const CHALLENGE_MS = 2 * 60_000;

/** A one-time value for the account's device to sign, good for two minutes. */
export async function issueChallenge(
  db: Kysely<Tables>,
  userId: string,
): Promise<Challenge> {
  const nonce = randomBytes(32).toString("hex");
  const expiresAt = new Date(Date.now() + CHALLENGE_MS);
  await pruneChallenges(db);
  await db
    .insertInto("challenges")
    .values({ nonce, user_id: userId, expires_at: expiresAt })
    .execute();
  return { nonce, expiresAt: expiresAt.toISOString() };
}

/** Uses up the account's challenge, refusing one that is unknown, already used or expired. */
export async function consumeChallenge(
  db: Kysely<Tables>,
  userId: string,
  nonce: string,
): Promise<void> {
  const challenge = await db
    .deleteFrom("challenges")
    .where("nonce", "=", nonce)
    .where("user_id", "=", userId)
    .where("expires_at", ">", sql<Date>`now()`)
    .returning("nonce")
    .executeTakeFirst();
  if (!challenge)
    throw new ApiFailure(
      400,
      "bad_request",
      "The challenge is unknown, already used, or expired.",
    );
}

/** Deletes challenges that expired over an hour ago, returning how many. */
export async function pruneChallenges(db: Kysely<Tables>): Promise<number> {
  const { numDeletedRows } = await db
    .deleteFrom("challenges")
    .where("expires_at", "<", sql<Date>`now() - interval '1 hour'`)
    .executeTakeFirst();
  return Number(numDeletedRows);
}
