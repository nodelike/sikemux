import type { Kysely, Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import { ApiFailure } from "../http.ts";
import type { Logger } from "../log.ts";
import type { ProviderEvent } from "./provider.ts";

/** The provider told of something the API has not heard of yet; it sends the event again later. */
function notYet(what: string): ApiFailure {
  return new ApiFailure(503, "unavailable", `${what} is not known yet.`);
}

async function subscriptionOf(
  trx: Transaction<Tables>,
  providerSubscriptionId: string,
) {
  const row = await trx
    .selectFrom("subscriptions")
    .select(["id", "customer_id", "status", "provider_updated_at"])
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .forUpdate()
    .executeTakeFirst();
  if (!row) throw notYet("The subscription");
  return row;
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
      const sub = event.subscription;
      await trx
        .insertInto("subscriptions")
        .values({
          customer_id: customer.id,
          provider_subscription_id: sub.providerSubscriptionId,
          product: sub.product,
          interval: sub.interval,
          quantity: sub.quantity,
          status: "active",
          current_period_end: sub.currentPeriodEnd,
          cancel_at_period_end: sub.cancelAtPeriodEnd,
          started_at: at,
          ended_at: null,
          provider_updated_at: at,
        })
        .onConflict((oc) => oc.column("provider_subscription_id").doNothing())
        .execute();
      return customer.id;
    }
    case "subscription.changed": {
      const sub = event.subscription;
      const held = await subscriptionOf(trx, sub.providerSubscriptionId);
      if (held.status !== "ended" && held.provider_updated_at <= at)
        await trx
          .updateTable("subscriptions")
          .set({
            product: sub.product,
            interval: sub.interval,
            quantity: sub.quantity,
            current_period_end: sub.currentPeriodEnd,
            cancel_at_period_end: sub.cancelAtPeriodEnd,
            provider_updated_at: at,
          })
          .where("id", "=", held.id)
          .execute();
      return held.customer_id;
    }
    case "subscription.lapsed":
    case "subscription.ended": {
      const held = await subscriptionOf(trx, event.providerSubscriptionId);
      const ended = event.type === "subscription.ended";
      if (held.status !== "ended" && (ended || held.provider_updated_at <= at))
        await trx
          .updateTable("subscriptions")
          .set(
            ended
              ? { status: "ended", ended_at: at, provider_updated_at: at }
              : { status: "lapsed", provider_updated_at: at },
          )
          .where("id", "=", held.id)
          .execute();
      return held.customer_id;
    }
    case "payment.made":
    case "payment.failed": {
      const held = await subscriptionOf(trx, event.providerSubscriptionId);
      const paid = event.type === "payment.made";
      await trx
        .insertInto("invoices")
        .values({
          customer_id: held.customer_id,
          subscription_id: held.id,
          provider_payment_id: event.providerPaymentId,
          product: event.product,
          quantity: event.quantity,
          reason: event.reason,
          amount: event.total.amount,
          currency: event.total.currency,
          status: paid ? "paid" : "failed",
          at,
          refunded_at: null,
        })
        .onConflict((oc) => oc.column("provider_payment_id").doNothing())
        .execute();
      if (paid && held.status === "lapsed" && held.provider_updated_at <= at)
        await trx
          .updateTable("subscriptions")
          .set({ status: "active", provider_updated_at: at })
          .where("id", "=", held.id)
          .execute();
      return held.customer_id;
    }
    case "payment.refunded": {
      const invoice = await trx
        .updateTable("invoices")
        .set({ status: "refunded", refunded_at: at })
        .where("provider_payment_id", "=", event.providerPaymentId)
        .where("status", "=", "paid")
        .returning("customer_id")
        .executeTakeFirst();
      if (invoice) return invoice.customer_id;
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
