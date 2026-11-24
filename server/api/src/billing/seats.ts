import type { OrgRole } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { previewSeats } from "./team.ts";

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
 * How many seats the org may fill: the fewest of what its active Team plan bills now, what waits
 * at renewal, and a cut its owner claimed that the provider has not reported yet. With no plan,
 * what the preview gives it, or 0.
 */
export async function seatLimit(
  db: Kysely<Tables>,
  orgId: string,
): Promise<number> {
  const row = await orgSubscriptions(db, orgId)
    .select(
      sql<number>`least(subscriptions.quantity, subscriptions.next_quantity, subscriptions.seats_cut_to)`.as(
        "seats",
      ),
    )
    .where("subscriptions.product", "=", "team")
    .where("subscriptions.status", "=", "active")
    .executeTakeFirst();
  return row?.seats ?? (await previewSeats(db, orgId));
}

/** Longer than a change takes at the provider, so an event this much later already shows it. */
const CLAIM_SETTLES = "5 minutes";

/**
 * Clears an owner's claim once the provider reports it: its snapshot, taken after the claim, holds
 * the org to `seats` or fewer, or was taken well after it and so shows the provider never kept it.
 */
export function claimSettledBy(seats: number, at: Date) {
  const settled = sql<boolean>`((${seats} <= seats_cut_to and seats_cut_at <= ${at}::timestamptz) or seats_cut_at < ${at}::timestamptz - interval '${sql.raw(CLAIM_SETTLES)}')`;
  return {
    seats_cut_to: sql<
      number | null
    >`case when ${settled} then null else seats_cut_to end`,
    seats_cut_at: sql<Date | null>`case when ${settled} then null else seats_cut_at end`,
  };
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
