import type { OrgRole } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

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

/**
 * How many seats the org may fill: its active Team plan's quantity, or a cut its owner took that
 * the provider has not applied yet, or 0 with no plan.
 */
export async function seatLimit(
  db: Kysely<Tables>,
  orgId: string,
): Promise<number> {
  const row = await orgSubscriptions(db, orgId)
    .select(
      sql<number>`least(subscriptions.quantity, coalesce(subscriptions.seats_cut_to, subscriptions.quantity))`.as(
        "seats",
      ),
    )
    .where("subscriptions.product", "=", "team")
    .where("subscriptions.status", "=", "active")
    .executeTakeFirst();
  return row?.seats ?? 0;
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

export const openInvites = (db: Kysely<Tables>, orgId: string) =>
  db
    .selectFrom("org_invites")
    .where("org_id", "=", orgId)
    .where("accepted_at", "is", null)
    .where("revoked_at", "is", null)
    .where("expires_at", ">", sql<Date>`now()`);

export async function memberCount(
  db: Kysely<Tables>,
  orgId: string,
  role?: OrgRole,
) {
  let query = db.selectFrom("org_users").where("org_id", "=", orgId);
  if (role) query = query.where("role", "=", role);
  const { count } = await query
    .select((eb) => eb.fn.countAll<string>().as("count"))
    .executeTakeFirstOrThrow();
  return Number(count);
}

/** Members and pending invites each hold a seat. */
export async function seatsHeld(db: Kysely<Tables>, orgId: string) {
  const { count } = await openInvites(db, orgId)
    .select((eb) => eb.fn.countAll<string>().as("count"))
    .executeTakeFirstOrThrow();
  return (await memberCount(db, orgId)) + Number(count);
}

/**
 * Locks the org against every other change to its members, invites and seats until the
 * transaction ends, and answers how many seats are held.
 */
export async function lockSeats(db: Kysely<Tables>, orgId: string) {
  await db
    .selectFrom("orgs")
    .select("id")
    .where("id", "=", orgId)
    .forUpdate()
    .execute();
  return seatsHeld(db, orgId);
}
