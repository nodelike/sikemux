import { randomUUID } from "node:crypto";
import type { Kysely } from "kysely";

import type { Tables } from "../src/db.ts";

/** Gives an org a paid Team plan of `seats` seats, as if its checkout had finished; answers its id. */
export async function teamPlan(
  db: Kysely<Tables>,
  orgId: string,
  seats: number,
): Promise<string> {
  const customer = await db
    .insertInto("billing_customers")
    .values({ org_id: orgId, provider: "stand-in" })
    .returning("id")
    .executeTakeFirstOrThrow();
  const now = new Date();
  const { id } = await db
    .insertInto("subscriptions")
    .values({
      customer_id: customer.id,
      provider_subscription_id: `sub_${randomUUID()}`,
      product: "team",
      interval: "month",
      quantity: seats,
      status: "active",
      current_period_end: new Date(now.getTime() + 30 * 24 * 60 * 60_000),
      cancel_at_period_end: false,
      started_at: now,
      ended_at: null,
      provider_updated_at: now,
    })
    .returning("id")
    .executeTakeFirstOrThrow();
  return id;
}
