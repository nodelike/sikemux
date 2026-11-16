import type { BillingProductId, Money } from "@sikemux/protocol";
import type { Kysely, Selectable } from "kysely";

import type { Tables } from "../db.ts";
import { ApiFailure } from "../http.ts";
import { callLater, type ProviderCall } from "./calls.ts";
import { REFUND_DAYS, product } from "./catalog.ts";
import { heldSubscription } from "./provider.ts";

const DAY_MS = 24 * 60 * 60_000;

type InvoiceRow = Selectable<Tables["invoices"]>;

/** A plan a payer gets one refund for: Pro, Team, or the Shed's boxes. */
function familyOf(id: BillingProductId) {
  const sold = product(id);
  return sold.kind === "shed" ? "shed" : sold.id;
}

/** Where a plan's refund stands: open until a time, asked for, or neither. */
export interface PlanRefund {
  until?: Date;
  requestedAt?: Date;
  /** What the refund returns: every payment on the plan in its first days. */
  amount?: Money;
  /** The payments it returns, while it may still be asked for. */
  covers: InvoiceRow[];
}

const NONE: PlanRefund = { covers: [] };

/**
 * A plan's refund. It is open for `REFUND_DAYS` from the plan's first payment and returns every
 * payment on the plan in that time, once per payer for each kind of plan.
 */
export async function planRefund(
  db: Kysely<Tables>,
  customerId: string,
  providerSubscriptionId: string,
  now = Date.now(),
): Promise<PlanRefund> {
  const invoices = await db
    .selectFrom("invoices")
    .selectAll()
    .where("customer_id", "=", customerId)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .orderBy("at")
    .execute();
  const first = invoices.find((invoice) => invoice.reason === "first_payment");
  if (!first) return NONE;
  const until = new Date(first.at.getTime() + REFUND_DAYS * DAY_MS);
  const covered = invoices.filter(
    (invoice) =>
      invoice.status === "paid" &&
      invoice.currency === first.currency &&
      invoice.at <= until,
  );
  const amount = {
    amount: covered.reduce((sum, invoice) => sum + Number(invoice.amount), 0),
    currency: first.currency,
  };
  const pending = covered.filter((invoice) => invoice.refund_requested_at);
  const [asked] = pending;
  if (asked?.refund_requested_at)
    return { requestedAt: asked.refund_requested_at, amount, covers: [] };
  if (first.status !== "paid" || until.getTime() <= now) return NONE;
  const [line] = first.lines;
  if (!line) return NONE;
  if (await refundedBefore(db, first, familyOf(line.product))) return NONE;
  return { until, amount, covers: covered };
}

/** Whether the payer already had a refund for this kind of plan on another subscription. */
async function refundedBefore(
  db: Kysely<Tables>,
  first: InvoiceRow,
  family: string,
) {
  const payer = await db
    .selectFrom("billing_customers")
    .select(["user_id", "org_id"])
    .where("id", "=", first.customer_id)
    .executeTakeFirstOrThrow();
  const earlier = await db
    .selectFrom("invoices")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "invoices.customer_id",
    )
    .select("invoices.lines")
    .where(
      payer.user_id
        ? (eb) => eb("billing_customers.user_id", "=", payer.user_id)
        : (eb) => eb("billing_customers.org_id", "=", payer.org_id),
    )
    .where("invoices.reason", "=", "first_payment")
    .where("invoices.refund_requested_at", "is not", null)
    .where(
      "invoices.provider_subscription_id",
      "!=",
      first.provider_subscription_id,
    )
    .execute();
  return earlier.some(({ lines }) =>
    lines.some((line) => familyOf(line.product) === family),
  );
}

/** Locks every billing customer of a payer, so two refund requests take turns. */
async function lockPayer(trx: Kysely<Tables>, customerId: string) {
  const payer = await trx
    .selectFrom("billing_customers")
    .select(["user_id", "org_id"])
    .where("id", "=", customerId)
    .executeTakeFirstOrThrow();
  await trx
    .selectFrom("billing_customers")
    .select("id")
    .where(
      payer.user_id
        ? (eb) => eb("user_id", "=", payer.user_id)
        : (eb) => eb("org_id", "=", payer.org_id),
    )
    .forUpdate()
    .execute();
}

/**
 * Asks for a plan's refund: its payments are marked asked, and the provider is told to stop the
 * plan renewing and to refund each one. When the refund is paid, the plan ends.
 */
export async function askRefund(
  db: Kysely<Tables>,
  provider: string,
  customerId: string,
  providerSubscriptionId: string,
): Promise<"asked" | "already"> {
  return db.transaction().execute(async (trx) => {
    await lockPayer(trx, customerId);
    const refund = await planRefund(trx, customerId, providerSubscriptionId);
    if (refund.requestedAt) return "already";
    if (!refund.covers.length)
      throw new ApiFailure(
        409,
        "conflict",
        `A plan is refunded once, within ${REFUND_DAYS} days of first paying for it.`,
      );
    await trx
      .updateTable("invoices")
      .set({ refund_requested_at: new Date() })
      .where(
        "id",
        "in",
        refund.covers.map((invoice) => invoice.id),
      )
      .execute();
    const renewing = await trx
      .selectFrom("subscriptions")
      .innerJoin(
        "billing_customers",
        "billing_customers.id",
        "subscriptions.customer_id",
      )
      .selectAll("subscriptions")
      .select("billing_customers.provider_customer_id")
      .where("subscriptions.customer_id", "=", customerId)
      .where(
        "subscriptions.provider_subscription_id",
        "=",
        providerSubscriptionId,
      )
      .where("subscriptions.status", "<>", "ended")
      .where("subscriptions.cancel_at_period_end", "=", false)
      .execute();
    const calls: ProviderCall[] = [
      ...renewing.map((row) => ({
        action: "cancel_at_period_end" as const,
        subscription: heldSubscription(row, row.provider_customer_id),
      })),
      ...refund.covers.map((invoice) => ({
        action: "refund" as const,
        providerPaymentId: invoice.provider_payment_id,
      })),
    ];
    await callLater(trx, provider, calls);
    return "asked";
  });
}

/** Whether a refund was asked for on the plan and the provider has not settled it yet. */
export async function refundPending(
  db: Kysely<Tables>,
  customerId: string,
  providerSubscriptionId: string,
) {
  const pending = await db
    .selectFrom("invoices")
    .select("id")
    .where("customer_id", "=", customerId)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .where("status", "=", "paid")
    .where("refund_requested_at", "is not", null)
    .executeTakeFirst();
  return Boolean(pending);
}
