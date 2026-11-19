import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import type { ClerkBackend } from "./clerk.ts";
import { dueSignOuts, endSignOut } from "../devices/sign-outs.ts";
import { deleteInClerk, revokeRemovedSession } from "./deletion.ts";
import { retryEndedSessions } from "./sign-ins.ts";

const BATCH = 20;

export async function sweepClerk(
  db: Kysely<Tables>,
  clerk: ClerkBackend,
  proofKey: Buffer | null,
  log: Logger,
): Promise<void> {
  const users = await db
    .selectFrom("users")
    .select("id")
    .where("deleted_at", "is not", null)
    .where("clerk_deleted_at", "is", null)
    .where("clerk_retry_at", "<=", sql<Date>`now()`)
    .orderBy("clerk_retry_at")
    .limit(BATCH)
    .execute();
  for (const { id } of users) await deleteInClerk(db, clerk, log, id);

  const sessions = await db
    .selectFrom("removed_devices")
    .select("key")
    .where("clerk_session_id", "is not", null)
    .where("clerk_revoked_at", "is", null)
    .where("clerk_retry_at", "<=", sql<Date>`now()`)
    .orderBy("clerk_retry_at")
    .limit(BATCH)
    .execute();
  for (const { key } of sessions)
    await revokeRemovedSession(db, clerk, log, key);

  for (const id of await dueSignOuts(db, BATCH))
    await endSignOut(db, clerk, log, id);

  await retryEndedSessions(db, clerk, proofKey, log, BATCH);
}
