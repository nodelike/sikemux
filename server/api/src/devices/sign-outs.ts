import { sql, type Kysely, type Transaction } from "kysely";

import type { ClerkBackend } from "../account/clerk.ts";
import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import { retryLater } from "../retry.ts";

/**
 * Records that removing a host signs out every other sign-in of the account, phones included, so
 * none left signed in can sign that Mac in again. Only `keptSession`, the remover's, stays.
 */
export async function signOutAfterHostRemoval(
  trx: Transaction<Tables>,
  userId: string,
  keptSession: string | null,
): Promise<string> {
  const { id } = await trx
    .insertInto("host_removal_sign_outs")
    .values({ user_id: userId, kept_session: keptSession })
    .returning("id")
    .executeTakeFirstOrThrow();
  return id;
}

/**
 * Ends at Clerk the sessions a host's removal signs out: those made before it, other than the one
 * that removed it. A failure is recorded for the sweeper to retry until done.
 */
export async function endSignOut(
  db: Kysely<Tables>,
  clerk: ClerkBackend,
  log: Logger,
  id: string,
): Promise<void> {
  await db.transaction().execute(async (trx) => {
    const row = await trx
      .selectFrom("host_removal_sign_outs")
      .selectAll()
      .where("id", "=", id)
      .where("done_at", "is", null)
      .forUpdate()
      .skipLocked()
      .executeTakeFirst();
    if (!row) return;
    let ended = 0;
    try {
      for (const session of await clerk.sessions(row.user_id)) {
        if (!session.live || session.id === row.kept_session) continue;
        if (session.createdAt > row.removed_at) continue;
        await clerk.revokeSession(session.id);
        ended += 1;
      }
    } catch (error) {
      const attempts = row.clerk_attempts + 1;
      await trx
        .updateTable("host_removal_sign_outs")
        .set({
          clerk_attempts: attempts,
          clerk_retry_at: retryLater(
            log,
            {
              service: "Clerk",
              what: "sign the account out after a host's removal",
            },
            { error, attempts, userId: row.user_id },
          ),
        })
        .where("id", "=", id)
        .execute();
      return;
    }
    await trx
      .updateTable("host_removal_sign_outs")
      .set({ done_at: sql<Date>`clock_timestamp()` })
      .where("id", "=", id)
      .execute();
    log.info(
      { userId: row.user_id, sessions: ended },
      "signed the account out after a host's removal",
    );
  });
}

/** The sign-outs due another try, oldest first. */
export async function dueSignOuts(
  db: Kysely<Tables>,
  limit: number,
): Promise<string[]> {
  const rows = await db
    .selectFrom("host_removal_sign_outs")
    .select("id")
    .where("done_at", "is", null)
    .where("clerk_retry_at", "<=", sql<Date>`now()`)
    .orderBy("clerk_retry_at")
    .limit(limit)
    .execute();
  return rows.map((row) => row.id);
}
