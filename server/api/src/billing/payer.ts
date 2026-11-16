import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { callLater } from "./calls.ts";
import { heldSubscription } from "./provider.ts";

/**
 * After `userId` stops being an owner of the org: if they paid for its plans, nobody pays with
 * their card from now on. Each plan is set to end at the period's end, or at once when it has
 * lapsed, so the provider's retry of a failed payment charges nobody. The calls are made later.
 */
export async function payerLeft(
  trx: Kysely<Tables>,
  orgId: string,
  userId: string,
): Promise<void> {
  const customers = await trx
    .updateTable("billing_customers")
    .set({ paid_by: null })
    .where("org_id", "=", orgId)
    .where("paid_by", "=", userId)
    .returning(["id", "provider", "provider_customer_id"])
    .execute();
  for (const customer of customers) {
    const open = await trx
      .selectFrom("subscriptions")
      .selectAll()
      .where("customer_id", "=", customer.id)
      .where("status", "<>", "ended")
      .where((eb) =>
        eb.or([
          eb("cancel_at_period_end", "=", false),
          eb("status", "=", "lapsed"),
        ]),
      )
      .execute();
    await callLater(
      trx,
      customer.provider,
      open.map((row) => ({
        action: row.status === "lapsed" ? "cancel_now" : "cancel_at_period_end",
        subscription: heldSubscription(row, customer.provider_customer_id),
      })),
    );
  }
}
