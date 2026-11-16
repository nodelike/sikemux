import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";

function orgSubscriptions(db: Kysely<Tables>, orgId: string) {
  return db
    .selectFrom("subscriptions")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "subscriptions.customer_id",
    )
    .where("billing_customers.org_id", "=", orgId);
}

/** How many seats the org pays for: its active Team plan's quantity, or 0 with none. */
export async function seatLimit(
  db: Kysely<Tables>,
  orgId: string,
): Promise<number> {
  const row = await orgSubscriptions(db, orgId)
    .select("subscriptions.quantity")
    .where("subscriptions.product", "=", "team")
    .where("subscriptions.status", "=", "active")
    .executeTakeFirst();
  return row?.quantity ?? 0;
}

/** Whether any of the org's subscriptions has not ended, a lapsed one still billed included. */
export async function hasOpenSubscriptions(
  db: Kysely<Tables>,
  orgId: string,
): Promise<boolean> {
  const row = await orgSubscriptions(db, orgId)
    .select("subscriptions.id")
    .where("subscriptions.status", "<>", "ended")
    .executeTakeFirst();
  return row !== undefined;
}
