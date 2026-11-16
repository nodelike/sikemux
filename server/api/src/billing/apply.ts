import type { InvoiceLine } from "@sikemux/protocol";
import type { Kysely, Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import { ApiFailure } from "../http.ts";
import type { Logger } from "../log.ts";
import type {
  ProviderEvent,
  ProviderItem,
  ProviderSubscription,
} from "./provider.ts";

/** The provider told of something the API has not heard of yet; it sends the event again later. */
function notYet(what: string): ApiFailure {
  return new ApiFailure(503, "unavailable", `${what} is not known yet.`);
}

/** The rows of one provider subscription, one per product, locked for the event. */
async function rowsOf(
  trx: Transaction<Tables>,
  provider: string,
  providerSubscriptionId: string,
) {
  const rows = await trx
    .selectFrom("subscriptions")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "subscriptions.customer_id",
    )
    .select([
      "subscriptions.id",
      "subscriptions.customer_id",
      "subscriptions.product",
      "subscriptions.status",
      "subscriptions.provider_updated_at",
    ])
    .where("billing_customers.provider", "=", provider)
    .where(
      "subscriptions.provider_subscription_id",
      "=",
      providerSubscriptionId,
    )
    .orderBy("subscriptions.started_at")
    .forUpdate("subscriptions")
    .execute();
  const [first] = rows;
  if (!first) throw notYet("The subscription");
  return { rows, customerId: first.customer_id };
}

type Row = Awaited<ReturnType<typeof rowsOf>>["rows"][number];

function isNewer(rows: readonly Row[], at: Date) {
  return rows.every((row) => row.provider_updated_at <= at);
}

function itemColumns(sub: ProviderSubscription, item: ProviderItem, at: Date) {
  return {
    product: item.product,
    interval: sub.interval,
    quantity: item.quantity,
    current_period_end: sub.currentPeriodEnd,
    cancel_at_period_end: sub.cancelAtPeriodEnd,
    next_product: item.atRenewal?.product ?? null,
    next_interval: item.atRenewal?.interval ?? null,
    next_quantity: item.atRenewal?.quantity ?? null,
    provider_updated_at: at,
  };
}

function insertItems(
  trx: Transaction<Tables>,
  customerId: string,
  sub: ProviderSubscription,
  items: readonly ProviderItem[],
  at: Date,
) {
  if (!items.length) return;
  return trx
    .insertInto("subscriptions")
    .values(
      items.map((item) => ({
        customer_id: customerId,
        provider_subscription_id: sub.providerSubscriptionId,
        status: "active" as const,
        started_at: at,
        ended_at: null,
        ...itemColumns(sub, item, at),
      })),
    )
    .onConflict((oc) =>
      oc.columns(["provider_subscription_id", "product"]).doNothing(),
    )
    .execute();
}

/**
 * Brings a subscription's rows to what the provider says it holds. A product that took another's
 * place, the only one gone and the only one new, keeps its row; any other product gone ends.
 */
async function changeItems(
  trx: Transaction<Tables>,
  rows: readonly Row[],
  customerId: string,
  sub: ProviderSubscription,
  at: Date,
) {
  const live = rows.filter((row) => row.status !== "ended");
  const gone = live.filter(
    (row) => !sub.items.some((item) => item.product === row.product),
  );
  const added = sub.items.filter(
    (item) => !rows.some((row) => row.product === item.product),
  );
  const [swappedRow] = gone.length === 1 && added.length === 1 ? gone : [];
  const pairs = sub.items.map((item) => ({
    item,
    row:
      rows.find((row) => row.product === item.product) ??
      (item === added[0] ? swappedRow : undefined),
  }));
  for (const { item, row } of pairs)
    if (row && row.status !== "ended")
      await trx
        .updateTable("subscriptions")
        .set(itemColumns(sub, item, at))
        .where("id", "=", row.id)
        .execute();
  await insertItems(
    trx,
    customerId,
    sub,
    pairs.filter(({ row }) => !row).map(({ item }) => item),
    at,
  );
  const ended = gone.filter((row) => row !== swappedRow).map((row) => row.id);
  if (ended.length)
    await trx
      .updateTable("subscriptions")
      .set({ status: "ended", ended_at: at, provider_updated_at: at })
      .where("id", "in", ended)
      .execute();
}

/** Applies one event and answers which customer it changed. */
async function apply(
  trx: Transaction<Tables>,
  provider: string,
  event: ProviderEvent,
): Promise<string> {
  const { at } = event;
  switch (event.type) {
    case "subscription.started": {
      const customer = await trx
        .updateTable("billing_customers")
        .set({ provider_customer_id: event.providerCustomerId })
        .where("id", "=", event.customerRef)
        .where("provider", "=", provider)
        .returning("id")
        .executeTakeFirst();
      if (!customer) throw notYet("The customer");
      await insertItems(
        trx,
        customer.id,
        event.subscription,
        event.subscription.items,
        at,
      );
      return customer.id;
    }
    case "subscription.changed": {
      const sub = event.subscription;
      const { rows, customerId } = await rowsOf(
        trx,
        provider,
        sub.providerSubscriptionId,
      );
      if (isNewer(rows, at)) await changeItems(trx, rows, customerId, sub, at);
      return customerId;
    }
    case "subscription.lapsed":
    case "subscription.ended": {
      const { rows, customerId } = await rowsOf(
        trx,
        provider,
        event.providerSubscriptionId,
      );
      const ended = event.type === "subscription.ended";
      const ids = rows
        .filter((row) => row.status !== "ended")
        .map((row) => row.id);
      if (ids.length && (ended || isNewer(rows, at)))
        await trx
          .updateTable("subscriptions")
          .set(
            ended
              ? { status: "ended", ended_at: at, provider_updated_at: at }
              : { status: "lapsed", provider_updated_at: at },
          )
          .where("id", "in", ids)
          .execute();
      return customerId;
    }
    case "payment.made":
    case "payment.failed": {
      const { rows, customerId } = await rowsOf(
        trx,
        provider,
        event.providerSubscriptionId,
      );
      const paid = event.type === "payment.made";
      await trx
        .insertInto("invoices")
        .values({
          customer_id: customerId,
          provider_subscription_id: event.providerSubscriptionId,
          provider_payment_id: event.providerPaymentId,
          lines: JSON.stringify(event.lines),
          reason: event.reason,
          amount: event.total.amount,
          currency: event.total.currency,
          status: paid ? "paid" : "failed",
          at,
          refund_requested_at: null,
          refunded_at: null,
        })
        .onConflict((oc) => oc.column("provider_payment_id").doNothing())
        .execute();
      const lapsed = rows
        .filter((row) => row.status === "lapsed")
        .map((row) => row.id);
      if (paid && lapsed.length && isNewer(rows, at))
        await trx
          .updateTable("subscriptions")
          .set({ status: "active", provider_updated_at: at })
          .where("id", "in", lapsed)
          .execute();
      return customerId;
    }
    case "payment.refunded": {
      const invoice = await trx
        .updateTable("invoices")
        .set({ status: "refunded", refunded_at: at })
        .where("provider_payment_id", "=", event.providerPaymentId)
        .where("status", "=", "paid")
        .returning([
          "customer_id",
          "reason",
          "provider_subscription_id",
          "lines",
        ])
        .executeTakeFirst();
      if (invoice) {
        if (invoice.reason === "first_payment")
          await endRefunded(trx, invoice, at);
        return invoice.customer_id;
      }
      const known = await trx
        .selectFrom("invoices")
        .select("customer_id")
        .where("provider_payment_id", "=", event.providerPaymentId)
        .executeTakeFirst();
      if (!known) throw notYet("The payment");
      return known.customer_id;
    }
  }
}

/** A refunded first payment takes back what it bought, though the provider ends it later. */
function endRefunded(
  trx: Transaction<Tables>,
  invoice: {
    customer_id: string;
    provider_subscription_id: string;
    lines: InvoiceLine[];
  },
  at: Date,
) {
  return trx
    .updateTable("subscriptions")
    .set({ status: "ended", ended_at: at, provider_updated_at: at })
    .where("customer_id", "=", invoice.customer_id)
    .where("provider_subscription_id", "=", invoice.provider_subscription_id)
    .where(
      "product",
      "in",
      invoice.lines.map((line) => line.product),
    )
    .where("status", "<>", "ended")
    .execute();
}

/** The people whose plan a customer's subscriptions decide: the person, or the org's members. */
export async function peopleOf(
  db: Kysely<Tables>,
  customerId: string,
): Promise<string[]> {
  const customer = await db
    .selectFrom("billing_customers")
    .select(["user_id", "org_id"])
    .where("id", "=", customerId)
    .executeTakeFirstOrThrow();
  if (customer.user_id) return [customer.user_id];
  if (!customer.org_id) return [];
  const members = await db
    .selectFrom("org_users")
    .select("user_id")
    .where("org_id", "=", customer.org_id)
    .execute();
  return members.map((member) => member.user_id);
}

/**
 * Applies a provider's events, each once: one already handled, even before a restart, is skipped.
 * Each goes in its own transaction with the plan.changed events for the people it concerns.
 */
export async function applyProviderEvents(
  db: Kysely<Tables>,
  provider: string,
  events: readonly ProviderEvent[],
  log: Logger,
): Promise<void> {
  for (const event of events) {
    await db.transaction().execute(async (trx) => {
      const fresh = await trx
        .insertInto("billing_events")
        .values({ provider, event_id: event.eventId, type: event.type })
        .onConflict((oc) => oc.columns(["provider", "event_id"]).doNothing())
        .returning("event_id")
        .executeTakeFirst();
      if (!fresh) return;
      const customerId = await apply(trx, provider, event);
      const people = await peopleOf(trx, customerId);
      await appendEvents(
        trx,
        people.map((userId) => ({ userId, type: "plan.changed" as const })),
      );
      log.info({ provider, type: event.type, customerId }, "billing event");
    });
  }
}
