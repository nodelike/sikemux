import { sql, type Kysely } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { pruneChallenges } from "../devices/challenge.ts";
import type { Logger } from "../log.ts";

/** How long each kind of history is kept. */
export const RETENTION_DAYS = {
  events: 400,
  removedDevices: 30,
  audit: 400,
  shares: 30,
} as const;
const BATCH = 5_000;

/**
 * Erases accounts deleted more than 30 days ago that Clerk has deleted too. Only a count of
 * their devices survives, in an audit row that names no one.
 */
export async function purgeAccounts(
  db: Kysely<Tables>,
  log: Logger,
): Promise<number> {
  const overdue = await db
    .selectFrom("users")
    .select("id")
    .where("purge_after", "<", sql<Date>`now() - interval '1 day'`)
    .where("clerk_deleted_at", "is", null)
    .execute();
  if (overdue.length)
    log.error(
      { accounts: overdue.length },
      "deleted accounts are due to be purged but Clerk has not deleted them yet",
    );

  const due = await db
    .selectFrom("users")
    .select("id")
    .where("purge_after", "<", sql<Date>`now()`)
    .where("clerk_deleted_at", "is not", null)
    .execute();
  let purged = 0;
  for (const { id } of due) {
    const done = await db.transaction().execute(async (trx) => {
      const user = await trx
        .selectFrom("users")
        .select("id")
        .where("id", "=", id)
        .where("purge_after", "<", sql<Date>`now()`)
        .forUpdate()
        .skipLocked()
        .executeTakeFirst();
      if (!user) return false;
      const removed = await trx
        .selectFrom("removed_devices")
        .select((eb) => eb.fn.countAll<string>().as("n"))
        .where("user_id", "=", id)
        .executeTakeFirstOrThrow();
      await trx.deleteFrom("events").where("user_id", "=", id).execute();
      await trx
        .deleteFrom("removed_devices")
        .where("user_id", "=", id)
        .execute();
      await trx.deleteFrom("audit").where("user_id", "=", id).execute();
      await trx.deleteFrom("challenges").where("user_id", "=", id).execute();
      await trx.deleteFrom("users").where("id", "=", id).execute();
      await audit(trx, {
        userId: null,
        actor: "system",
        action: "account.purged",
        detail: { devices: Number(removed.n) },
      });
      return true;
    });
    if (done) purged += 1;
  }
  if (purged) log.info({ accounts: purged }, "purged deleted accounts");
  return purged;
}

export interface Pruned {
  events: number;
  removedDevices: number;
  audit: number;
  shares: number;
  challenges: number;
}

async function inBatches(run: () => Promise<number>): Promise<number> {
  let total = 0;
  for (;;) {
    const deleted = await run();
    total += deleted;
    if (deleted < BATCH) return total;
  }
}

/**
 * Deletes history past its retention, a batch at a time. Each account remembers the newest event
 * pruned from it, so a device whose cursor is older is told to reset.
 */
export async function pruneHistory(
  db: Kysely<Tables>,
  log: Logger,
): Promise<Pruned> {
  const events = await inBatches(async () => {
    const { rows } = await sql<{ n: string }>`
      with gone as (
        delete from events where id in (
          select id from events
          where at < now() - make_interval(days => ${RETENTION_DAYS.events})
          order by id limit ${BATCH}
        )
        returning user_id, id
      ), bumped as (
        update users set events_pruned_through = greatest(events_pruned_through, latest)
        from (select user_id, max(id) as latest from gone group by user_id) as pruned
        where users.id = pruned.user_id
      )
      select count(*) as n from gone`.execute(db);
    return Number(rows[0]?.n ?? 0);
  });
  const olderThan = (
    table: "removed_devices" | "audit",
    column: "removed_at" | "at",
    days: number,
  ) =>
    inBatches(async () => {
      const { rows } = await sql<{ n: string }>`
        with gone as (
          delete from ${sql.table(table)} where ctid in (
            select ctid from ${sql.table(table)}
            where ${sql.ref(column)} < now() - make_interval(days => ${days})
            limit ${BATCH}
          )
          returning 1
        )
        select count(*) as n from gone`.execute(db);
      return Number(rows[0]?.n ?? 0);
    });
  const removedDevices = await olderThan(
    "removed_devices",
    "removed_at",
    RETENTION_DAYS.removedDevices,
  );
  const audit = await olderThan("audit", "at", RETENTION_DAYS.audit);
  const shares = await inBatches(async () => {
    const { rows } = await sql<{ n: string }>`
      with gone as (
        delete from shares where id in (
          select id from shares
          where coalesce(ended_at, expires_at) < now() - make_interval(days => ${RETENTION_DAYS.shares})
          limit ${BATCH}
        )
        returning 1
      )
      select count(*) as n from gone`.execute(db);
    return Number(rows[0]?.n ?? 0);
  });
  const challenges = await pruneChallenges(db);
  const pruned = { events, removedDevices, audit, shares, challenges };
  log.info(pruned, "pruned old history");
  return pruned;
}
