import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import type { Logger } from "../log.ts";

/**
 * Gives Pro free for `days` to every account that exists now and has not been deleted. An
 * account that already holds a gift keeps the one it has.
 */
export async function givePro(
  db: Kysely<Tables>,
  days: number,
  log: Logger,
): Promise<number> {
  const given = await db.transaction().execute(async (trx) => {
    const rows = await trx
      .insertInto("plan_gifts")
      .columns(["user_id", "plan", "ends_at"])
      .expression((eb) =>
        eb
          .selectFrom("users")
          .select([
            "id",
            sql<"pro">`'pro'`.as("plan"),
            sql<Date>`now() + make_interval(days => ${days})`.as("ends_at"),
          ])
          .where("deleted_at", "is", null),
      )
      .onConflict((oc) => oc.column("user_id").doNothing())
      .returning("user_id")
      .execute();
    await appendEvents(
      trx,
      rows.map((row) => ({
        userId: row.user_id,
        type: "plan.changed" as const,
      })),
    );
    return rows.length;
  });
  log.info({ accounts: given, days }, "gave Pro free");
  return given;
}

/** Ends the gifts whose date has passed, so the people who held them hear their plan changed. */
export async function endPlanGifts(
  db: Kysely<Tables>,
  log: Logger,
): Promise<void> {
  const ended = await db.transaction().execute(async (trx) => {
    const rows = await trx
      .deleteFrom("plan_gifts")
      .where("ends_at", "<=", sql<Date>`now()`)
      .returning("user_id")
      .execute();
    await appendEvents(
      trx,
      rows.map((row) => ({
        userId: row.user_id,
        type: "plan.changed" as const,
      })),
    );
    return rows.length;
  });
  if (ended > 0) log.info({ accounts: ended }, "ended plan gifts");
}
