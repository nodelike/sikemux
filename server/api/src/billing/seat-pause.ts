import type { SeatPause } from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import type { ProviderCall } from "./calls.ts";
import { heldSubscription, type BillingProvider } from "./provider.ts";

export type Pausing = Pick<
  BillingProvider,
  "name" | "pause" | "resumeOn" | "keepPaused"
>;

/**
 * Asks for each person's seats to be checked against their own Pro once this transaction commits.
 * Only people paying for Pro themselves get a check.
 */
export async function checkSeatPauses(
  trx: Transaction<Tables>,
  userIds: readonly string[],
): Promise<void> {
  if (!userIds.length) return;
  await trx
    .insertInto("billing_calls")
    .columns(["provider", "action", "user_id"])
    .expression(
      trx
        .selectFrom("subscriptions")
        .innerJoin(
          "billing_customers",
          "billing_customers.id",
          "subscriptions.customer_id",
        )
        .select([
          "billing_customers.provider",
          sql.lit("settle_seat_pause").as("action"),
          "billing_customers.user_id",
        ])
        .where("billing_customers.user_id", "in", userIds)
        .where("subscriptions.product", "=", "pro")
        .where("subscriptions.status", "<>", "ended"),
    )
    .execute();
}

/**
 * The orgs whose Team plan includes Pro for this person through their seat. A lapsed plan still
 * does, so a renewal that is late leaves their own Pro paused.
 */
export async function seatsIncludingPro(
  db: Kysely<Tables>,
  userId: string,
): Promise<{ id: string; name: string }[]> {
  return db
    .selectFrom("org_users")
    .innerJoin("orgs", "orgs.id", "org_users.org_id")
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
    .select(["orgs.id", "orgs.name"])
    .where("org_users.user_id", "=", userId)
    .where("subscriptions.product", "=", "team")
    .where("subscriptions.status", "in", ["active", "lapsed"])
    .distinct()
    .orderBy("orgs.name")
    .execute();
}

async function record(
  db: Kysely<Tables>,
  userId: string,
  subscriptionId: string,
  change: (trx: Transaction<Tables>) => Promise<{ numUpdatedRows: bigint }>,
  detail: Record<string, unknown>,
) {
  await db.transaction().execute(async (trx) => {
    if (!(await change(trx)).numUpdatedRows) return;
    await audit(trx, {
      userId,
      actor: "system",
      action: "billing.subscription_paused",
      subject: subscriptionId,
      detail: { why: "seat", ...detail },
    });
    await appendEvents(trx, [{ userId, type: "plan.changed" }]);
  });
}

/**
 * Stops the clock again on a Pro paused for a seat whose kept time had started to run: the provider
 * drops its resume, and the time still left is kept.
 */
export async function keepKeptTime(
  db: Kysely<Tables>,
  provider: Pick<BillingProvider, "keepPaused">,
  row: Parameters<typeof heldSubscription>[0] & {
    id: string;
    customer_id: string;
    resumes_at: Date | null;
    provider_customer_id: string | null;
  },
): Promise<void> {
  if (!row.resumes_at) return;
  await provider.keepPaused(heldSubscription(row, row.provider_customer_id));
  const customer = await db
    .selectFrom("billing_customers")
    .select("user_id")
    .where("id", "=", row.customer_id)
    .executeTakeFirstOrThrow();
  if (!customer.user_id) return;
  await record(
    db,
    customer.user_id,
    row.id,
    (trx) =>
      trx
        .updateTable("subscriptions")
        .set({
          paid_left: sql<string>`greatest(resumes_at - now(), interval '0')`,
          resumes_at: null,
        })
        .where("id", "=", row.id)
        .where("resumes_at", "is not", null)
        .executeTakeFirst(),
    { again: true },
  );
}

/**
 * Makes a person's own Pro match their seats: paused, with its paid time kept, while a seat
 * includes Pro; once none does, Pro runs on that kept time and the provider bills again only when
 * it is used up. A pause made anywhere else is never resumed here. Answers when to check again
 * when the provider asked to wait, as it does just before a renewal.
 */
export async function settleSeatPause(
  db: Kysely<Tables>,
  provider: Pausing,
  userId: string,
): Promise<Date | null> {
  const user = await db
    .selectFrom("users")
    .select("deleted_at")
    .where("id", "=", userId)
    .executeTakeFirst();
  if (!user || user.deleted_at) return null;
  const row = await db
    .selectFrom("subscriptions")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "subscriptions.customer_id",
    )
    .selectAll("subscriptions")
    .select([
      "billing_customers.provider_customer_id",
      sql<Date | null>`now() + subscriptions.paid_left`.as("runs_out_at"),
    ])
    .where("billing_customers.user_id", "=", userId)
    .where("billing_customers.provider", "=", provider.name)
    .where("subscriptions.product", "=", "pro")
    .where("subscriptions.status", "<>", "ended")
    .executeTakeFirst();
  if (!row) return null;
  const held = heldSubscription(row, row.provider_customer_id);
  const seated = (await seatsIncludingPro(db, userId)).length > 0;

  if (seated && !row.paused_for_seat_at) {
    if (row.cancel_at_period_end) return null;
    if (row.status !== "active" && row.status !== "paused") return null;
    const paused = await provider.pause(held);
    if ("askAgainAt" in paused) return paused.askAgainAt;
    await record(
      db,
      userId,
      row.id,
      (trx) =>
        trx
          .updateTable("subscriptions")
          .set({
            paused_for_seat_at: sql<Date>`now()`,
            paid_left: sql<string>`greatest(${paused.paidUntil}::timestamptz - now(), interval '0')`,
            resumes_at: null,
          })
          .where("id", "=", row.id)
          .where("paused_for_seat_at", "is", null)
          .executeTakeFirst(),
      { paidUntil: paused.paidUntil.toISOString() },
    );
  } else if (seated && row.resumes_at) {
    await keepKeptTime(db, provider, row);
  } else if (!seated && row.paused_for_seat_at && row.runs_out_at) {
    const at = row.runs_out_at;
    await provider.resumeOn(held, at);
    await db.transaction().execute(async (trx) => {
      const set = await trx
        .updateTable("subscriptions")
        .set({ resumes_at: at, paid_left: null })
        .where("id", "=", row.id)
        .where("resumes_at", "is", null)
        .executeTakeFirst();
      if (!set.numUpdatedRows) return;
      await audit(trx, {
        userId,
        actor: "system",
        action: "billing.subscription_resume_set",
        subject: row.id,
        detail: { at: at.toISOString() },
      });
      await appendEvents(trx, [{ userId, type: "plan.changed" }]);
    });
  }
  return null;
}

/**
 * The refund owed when a Pro paused for a seat is cancelled: the share of its last payment that
 * the paid time it still keeps is worth. Forgets that time, so it is refunded once.
 */
export async function refundKeptTime(
  trx: Transaction<Tables>,
  subscriptionId: string,
): Promise<ProviderCall | null> {
  const row = await trx
    .selectFrom("subscriptions")
    .select([
      "customer_id",
      "provider_subscription_id",
      "interval",
      sql<
        number | null
      >`extract(epoch from coalesce(greatest(resumes_at - now(), interval '0'), paid_left))::float8`.as(
        "kept_seconds",
      ),
    ])
    .where("id", "=", subscriptionId)
    .where("paused_for_seat_at", "is not", null)
    .forUpdate()
    .executeTakeFirst();
  if (!row?.kept_seconds) return null;
  await trx
    .updateTable("subscriptions")
    .set({ paid_left: null, resumes_at: null })
    .where("id", "=", subscriptionId)
    .execute();
  const paid = await trx
    .selectFrom("invoices")
    .select([
      "provider_payment_id",
      "amount",
      sql<number>`extract(epoch from (at + ${sql.lit(`1 ${row.interval}`)}::interval) - at)::float8`.as(
        "period_seconds",
      ),
    ])
    .where("customer_id", "=", row.customer_id)
    .where("provider_subscription_id", "=", row.provider_subscription_id)
    .where("reason", "in", ["first_payment", "renewal"])
    .where("status", "=", "paid")
    .orderBy("at", "desc")
    .limit(1)
    .executeTakeFirst();
  if (!paid) return null;
  const amount = Math.min(
    Number(paid.amount),
    Math.floor((Number(paid.amount) * row.kept_seconds) / paid.period_seconds),
  );
  return amount > 0
    ? { action: "refund", providerPaymentId: paid.provider_payment_id, amount }
    : null;
}

/** Where a Pro paused for a seat stands, as a plan shows it. */
export async function seatPauseOf(
  db: Kysely<Tables>,
  row: {
    customer_id: string;
    status: string;
    paused_for_seat_at: Date | null;
    resumes_at: Date | null;
  },
): Promise<SeatPause | undefined> {
  if (!row.paused_for_seat_at || row.status !== "paused") return undefined;
  const customer = await db
    .selectFrom("billing_customers")
    .select("user_id")
    .where("id", "=", row.customer_id)
    .executeTakeFirst();
  const orgs = customer?.user_id
    ? await seatsIncludingPro(db, customer.user_id)
    : [];
  return {
    orgs: orgs.map((org) => org.name),
    ...(row.resumes_at ? { resumesAt: row.resumes_at.toISOString() } : {}),
  };
}

/** Why a person with a seat cannot buy Pro of their own. */
export function seatIncludesPro(orgs: readonly { name: string }[]): string {
  const names = orgs.map((org) => org.name);
  const last = names.pop();
  return names.length
    ? `Your seats in ${names.join(", ")} and ${last} include Pro.`
    : `Your seat in ${last} includes Pro.`;
}
