import type { BillingProductId } from "@sikemux/protocol";
import type { Kysely } from "kysely";

import { isStaff } from "../account/staff.ts";
import type { Tables } from "../db.ts";
import { orgRole } from "../orgs/roles.ts";
import { product } from "./catalog.ts";
import { hasOpenSubscriptions, lockSeats } from "./seats.ts";

export interface Purchase {
  customerId: string;
  orgId: string | null;
  /** Who pays for the org's plans now, if anyone. */
  paidBy: string | null;
  openedBy: string;
  product: BillingProductId;
  quantity: number;
}

/**
 * Why a purchase is refused, or null when it may go ahead. Asked when a checkout opens and again
 * on what was paid, since a buyer can change their checkout on the provider's page. A second plan
 * is refused at checkout and, once paid, ended and refunded as a surplus.
 */
export async function refusal(
  trx: Kysely<Tables>,
  buying: Purchase,
): Promise<string | null> {
  const sold = product(buying.product);
  if (buying.quantity > sold.maxQuantity)
    return `${sold.name} is sold up to ${sold.maxQuantity} at once.`;
  if (sold.kind === "shed" && !(await isStaff(trx, buying.openedBy)))
    return "The Shed is not on sale yet: no box can be provisioned.";
  if (buying.orgId !== null) {
    if ((await orgRole(trx, buying.orgId, buying.openedBy)) !== "owner")
      return "Only the org's owner acts for its billing.";
    if (
      buying.paidBy !== buying.openedBy &&
      (await hasOpenSubscriptions(trx, buying.orgId))
    )
      return buying.paidBy
        ? "Another owner pays for this org's plans; they buy for it."
        : "The org's plans were paid for by someone who no longer owns it. Buy once they have ended.";
    if (sold.id === "team") {
      const held = await lockSeats(trx, buying.orgId);
      if (buying.quantity < held)
        return `${held} seats are held by members and pending invites; buy at least that many.`;
    }
  }
  return null;
}
