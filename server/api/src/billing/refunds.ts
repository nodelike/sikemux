import type { BillingProductId, Money } from "@sikemux/protocol";
import type { Kysely, Selectable } from "kysely";

import type { Tables } from "../db.ts";
import { ApiFailure } from "../http.ts";
import { callLater, type ProviderCall } from "./calls.ts";
import { REFUND_DAYS, product } from "./catalog.ts";
import { heldSubscription } from "./provider.ts";
import { checkSeatPauses } from "./seat-pause.ts";

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

export interface PlanRefundTarget {
  customerId: string;
  providerSubscriptionId: string;
}

export const refundTargetKey = (
  customerId: string,
  providerSubscriptionId: string,
) => `${customerId}:${providerSubscriptionId}`;

/**
 * Batched `planRefund` for many subscriptions/invoices at once without per-row queries.
 */
export async function planRefunds(
  db: Kysely<Tables>,
  provider: string | undefined,
  targets: readonly PlanRefundTarget[],
  now = Date.now(),
): Promise<Map<string, PlanRefund>> {
  const result = new Map<string, PlanRefund>();
  if (!provider || !targets.length) return result;

  const uniqueKeys = new Set<string>();
  const uniqueTargets: PlanRefundTarget[] = [];
  for (const target of targets) {
    const key = refundTargetKey(
      target.customerId,
      target.providerSubscriptionId,
    );
    if (!uniqueKeys.has(key)) {
      uniqueKeys.add(key);
      uniqueTargets.push(target);
    }
  }

  const customerIds = [...new Set(uniqueTargets.map((t) => t.customerId))];
  const customers = await db
    .selectFrom("billing_customers")
    .select(["id", "provider", "user_id", "org_id"])
    .where("id", "in", customerIds)
    .execute();
  const matchingCustomers = new Map(
    customers.filter((c) => c.provider === provider).map((c) => [c.id, c]),
  );
  const matchedTargets = uniqueTargets.filter((t) =>
    matchingCustomers.has(t.customerId),
  );
  if (!matchedTargets.length) return result;

  const matchedCustomerIds = [
    ...new Set(matchedTargets.map((t) => t.customerId)),
  ];
  const subIds = [
    ...new Set(matchedTargets.map((t) => t.providerSubscriptionId)),
  ];
  const allInvoices = await db
    .selectFrom("invoices")
    .selectAll()
    .where("customer_id", "in", matchedCustomerIds)
    .where("provider_subscription_id", "in", subIds)
    .orderBy("at")
    .execute();

  const invoicesByKey = new Map<string, InvoiceRow[]>();
  for (const invoice of allInvoices) {
    const key = refundTargetKey(
      invoice.customer_id,
      invoice.provider_subscription_id,
    );
    let list = invoicesByKey.get(key);
    if (!list) {
      list = [];
      invoicesByKey.set(key, list);
    }
    list.push(invoice);
  }

  interface Candidate {
    key: string;
    first: InvoiceRow;
    family: string;
    until: Date;
    amount: Money;
    covered: InvoiceRow[];
    payerKey: string;
  }

  const candidates: Candidate[] = [];
  const payerUserIds = new Set<string>();
  const payerOrgIds = new Set<string>();

  for (const target of matchedTargets) {
    const key = refundTargetKey(
      target.customerId,
      target.providerSubscriptionId,
    );
    const invoices = invoicesByKey.get(key) ?? [];
    const first = invoices.find(
      (invoice) => invoice.reason === "first_payment",
    );
    if (!first) continue;
    if (
      invoices.some(
        (invoice) =>
          invoice.kept_refund !== null &&
          !(invoice.kept_refund_refused_at && !invoice.kept_refunded_at),
      )
    )
      continue;
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
    const [asked] = covered.filter((invoice) => invoice.refund_requested_at);
    if (asked?.refund_requested_at) {
      result.set(key, {
        requestedAt: asked.refund_requested_at,
        amount,
        covers: [],
      });
      continue;
    }
    if (first.status !== "paid" || until.getTime() <= now) continue;
    const [line] = first.lines;
    if (!line) continue;
    const customer = matchingCustomers.get(target.customerId);
    if (!customer) continue;
    const payerKey = customer.user_id
      ? `user:${customer.user_id}`
      : `org:${customer.org_id}`;
    if (customer.user_id) payerUserIds.add(customer.user_id);
    else if (customer.org_id) payerOrgIds.add(customer.org_id);
    candidates.push({
      key,
      first,
      family: familyOf(line.product),
      until,
      amount,
      covered,
      payerKey,
    });
  }

  if (!candidates.length) return result;

  const priorRows = await db
    .selectFrom("invoices")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "invoices.customer_id",
    )
    .select([
      "billing_customers.user_id",
      "billing_customers.org_id",
      "invoices.provider_subscription_id",
      "invoices.lines",
    ])
    .where("billing_customers.provider", "=", provider)
    .where((eb) => {
      const clauses = [];
      if (payerUserIds.size)
        clauses.push(eb("billing_customers.user_id", "in", [...payerUserIds]));
      if (payerOrgIds.size)
        clauses.push(eb("billing_customers.org_id", "in", [...payerOrgIds]));
      return eb.or(clauses);
    })
    .where("invoices.reason", "=", "first_payment")
    .where("invoices.refund_requested_at", "is not", null)
    .execute();

  const priorByPayer = new Map<
    string,
    { subId: string; families: Set<string> }[]
  >();
  for (const row of priorRows) {
    const payerKey = row.user_id ? `user:${row.user_id}` : `org:${row.org_id}`;
    let list = priorByPayer.get(payerKey);
    if (!list) {
      list = [];
      priorByPayer.set(payerKey, list);
    }
    list.push({
      subId: row.provider_subscription_id,
      families: new Set(row.lines.map((line) => familyOf(line.product))),
    });
  }

  for (const candidate of candidates) {
    const prior = priorByPayer.get(candidate.payerKey) ?? [];
    const hadBefore = prior.some(
      (entry) =>
        entry.subId !== candidate.first.provider_subscription_id &&
        entry.families.has(candidate.family),
    );
    if (!hadBefore) {
      result.set(candidate.key, {
        until: candidate.until,
        amount: candidate.amount,
        covers: candidate.covered,
      });
    }
  }

  return result;
}

/**
 * A plan's refund. It is open for `REFUND_DAYS` from the plan's first payment and returns every
 * payment on the plan in that time, once per payer for each kind of plan. A plan paid through
 * another provider than the one set up is history only, and has none.
 */
export async function planRefund(
  db: Kysely<Tables>,
  provider: string | undefined,
  customerId: string,
  providerSubscriptionId: string,
  now = Date.now(),
): Promise<PlanRefund> {
  const map = await planRefunds(
    db,
    provider,
    [{ customerId, providerSubscriptionId }],
    now,
  );
  return map.get(refundTargetKey(customerId, providerSubscriptionId)) ?? NONE;
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
 * plan renewing and to refund each one. A Pro paused for a seat is left to its seat check, which
 * holds its paid time and drops any resume before it asks. When the refund is paid, the plan ends.
 */
export async function askRefund(
  db: Kysely<Tables>,
  provider: string,
  customerId: string,
  providerSubscriptionId: string,
): Promise<"asked" | "already"> {
  return db.transaction().execute(async (trx) => {
    await lockPayer(trx, customerId);
    const refund = await planRefund(
      trx,
      provider,
      customerId,
      providerSubscriptionId,
    );
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
    const open = await trx
      .selectFrom("subscriptions")
      .innerJoin(
        "billing_customers",
        "billing_customers.id",
        "subscriptions.customer_id",
      )
      .selectAll("subscriptions")
      .select([
        "billing_customers.provider_customer_id",
        "billing_customers.user_id",
      ])
      .where("subscriptions.customer_id", "=", customerId)
      .where(
        "subscriptions.provider_subscription_id",
        "=",
        providerSubscriptionId,
      )
      .where("subscriptions.status", "<>", "ended")
      .execute();
    const pausedForSeat = open.find((row) => row.paused_for_seat_at);
    if (pausedForSeat?.user_id) {
      await checkSeatPauses(trx, [pausedForSeat.user_id]);
      return "asked";
    }
    const calls: ProviderCall[] = [
      ...open
        .filter((row) => !row.cancel_at_period_end)
        .map((row) => ({
          action:
            row.status === "paused"
              ? ("cancel_now" as const)
              : ("cancel_at_period_end" as const),
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
    .where((eb) =>
      eb.or([
        eb("refund_requested_at", "is not", null),
        eb.and([
          eb("kept_refund", "is not", null),
          eb("kept_refunded_at", "is", null),
          eb("kept_refund_refused_at", "is", null),
        ]),
      ]),
    )
    .executeTakeFirst();
  return Boolean(pending);
}
