import type {
  Allowance,
  BillingProductId,
  PlanId,
  ShedBoxes,
} from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { ALL_FEATURES, featuresOf, product } from "./catalog.ts";

type Payer = { userId: string } | { orgId: string };

/** The products a payer pays for now, with their quantities. Lapsed and ended ones count for nothing. */
async function paidFor(db: Kysely<Tables>, payer: Payer) {
  return db
    .selectFrom("subscriptions")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "subscriptions.customer_id",
    )
    .select(["subscriptions.product", "subscriptions.quantity"])
    .where("subscriptions.status", "=", "active")
    .where(
      "userId" in payer
        ? "billing_customers.user_id"
        : "billing_customers.org_id",
      "=",
      "userId" in payer ? payer.userId : payer.orgId,
    )
    .execute();
}

function boxesOf(
  rows: readonly { product: BillingProductId; quantity: number }[],
): ShedBoxes[] {
  const counts = new Map<BillingProductId, number>();
  for (const row of rows)
    if (product(row.product).kind === "shed")
      counts.set(row.product, (counts.get(row.product) ?? 0) + row.quantity);
  return [...counts].map(([id, quantity]) => ({ product: id, quantity }));
}

/** The orgs whose paid Team plan `userId` holds a seat in. */
async function seatOrgs(db: Kysely<Tables>, userId: string): Promise<string[]> {
  const rows = await db
    .selectFrom("org_users")
    .innerJoin(
      "billing_customers",
      "billing_customers.org_id",
      "org_users.org_id",
    )
    .innerJoin(
      "subscriptions",
      "subscriptions.customer_id",
      "billing_customers.id",
    )
    .select("org_users.org_id")
    .where("org_users.user_id", "=", userId)
    .where("subscriptions.product", "=", "team")
    .where("subscriptions.status", "=", "active")
    .distinct()
    .execute();
  return rows.map((row) => row.org_id);
}

/** When the Pro a person was given free ends, while it has not. */
async function giftEnds(db: Kysely<Tables>, userId: string) {
  const gift = await db
    .selectFrom("plan_gifts")
    .select("ends_at")
    .where("user_id", "=", userId)
    .where("ends_at", ">", sql<Date>`now()`)
    .executeTakeFirst();
  return gift?.ends_at;
}

/**
 * A person's or an org's plan and what it may use: Pro's features for Pro, paid or given free
 * until a date, Team's through a seat in a paid org (a seat includes Pro), and the Shed boxes it
 * bought. Until billing is live, everyone may use everything; the plan still says what was bought.
 */
export async function allowance(
  db: Kysely<Tables>,
  payer: Payer,
  { live }: { live: boolean },
): Promise<Allowance> {
  const rows = await paidFor(db, payer);
  const orgs = "userId" in payer ? await seatOrgs(db, payer.userId) : [];
  const owns = (id: BillingProductId) => rows.some((row) => row.product === id);
  const paid: PlanId =
    owns("team") || orgs.length > 0 ? "team" : owns("pro") ? "pro" : "free";
  const gift =
    paid === "free" && "userId" in payer
      ? await giftEnds(db, payer.userId)
      : undefined;
  const plan = gift ? "pro" : paid;
  return {
    plan,
    features: [...(live ? featuresOf(plan) : ALL_FEATURES)],
    boxes: boxesOf(rows),
    ...("userId" in payer ? { seatOrgs: orgs } : {}),
    ...(gift ? { giftEnds: gift.toISOString() } : {}),
  };
}

/** The plan a person holds, for the account and its events. */
export async function planOf(
  db: Kysely<Tables>,
  userId: string,
): Promise<PlanId> {
  return (await allowance(db, { userId }, { live: true })).plan;
}
