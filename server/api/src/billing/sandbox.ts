import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import { PADDLE_SANDBOX } from "./paddle.ts";

/** Plans bought with a sandbox key end once a live key is in, so they grant nothing. */
export async function endSandboxPlans(
  db: Kysely<Tables>,
  log: Logger,
): Promise<void> {
  const ended = await db
    .updateTable("subscriptions")
    .set({ status: "ended", ended_at: sql<Date>`now()` })
    .where("status", "<>", "ended")
    .where("customer_id", "in", (eb) =>
      eb
        .selectFrom("billing_customers")
        .select("id")
        .where("provider", "=", PADDLE_SANDBOX),
    )
    .executeTakeFirst();
  if (ended.numUpdatedRows)
    log.info(
      { ended: Number(ended.numUpdatedRows) },
      "ended the plans bought with Paddle's sandbox",
    );
}
