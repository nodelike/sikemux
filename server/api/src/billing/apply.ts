import type { InvoiceLine } from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import { ApiFailure } from "../http.ts";
import type { Logger } from "../log.ts";
import { PLAN_PRODUCTS, product } from "./catalog.ts";
import type {
  BillingProvider,
  ProviderEvent,
  ProviderItem,
  ProviderSubscription,
} from "./provider.ts";

/**
 * How long an event about something unknown is answered "not yet", for the provider to send again
 * once what it follows has arrived. Past that, what it is about was erased with its account.
 */
const GONE_AFTER_MS = 24 * 60 * 60_000;

/** An event about something erased, which is acknowledged and changes nothing. */
class Gone extends Error {}

function unknown(what: string, at: Date): Error {
  if (Date.now() - at.getTime() > GONE_AFTER_MS) return new Gone(what);
  return new ApiFailure(503, "unavailable", `${what} is not known yet.`);
}

type Acting = Pick<BillingProvider, "name" | "cancel" | "refund">;

interface Applied {
  customerId: string;
  /** What to ask of the provider once the event is recorded. */
  afterwards?: (provider: Acting) => Promise<void>;
}

/** The rows of one provider subscription, one per product, locked for the event. */
async function rowsOf(
  trx: Transaction<Tables>,
  provider: string,
  providerSubscriptionId: string,
  at: Date,
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
      "subscriptions.started_at",
      "subscriptions.ended_at",
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
  if (!first) throw unknown("The subscription", at);
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
  { surplus = false } = {},
) {
  if (!items.length) return;
  return trx
    .insertInto("subscriptions")
    .values(
      items.map((item) => ({
        customer_id: customerId,
        provider_subscription_id: sub.providerSubscriptionId,
        status: surplus ? ("ended" as const) : ("active" as const),
        started_at: at,
        ended_at: surplus ? at : null,
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

/**
 * A second plan for one payer, as two checkouts finishing together make: it is kept as a
 * subscription that ended as it started, and ended with the provider and refunded.
 */
async function isSurplus(
  trx: Transaction<Tables>,
  customerId: string,
  sub: ProviderSubscription,
) {
  if (!sub.items.some((item) => product(item.product).kind === "plan"))
    return false;
  const plan = await trx
    .selectFrom("subscriptions")
    .select("id")
    .where("customer_id", "=", customerId)
    .where("status", "<>", "ended")
    .where("product", "in", PLAN_PRODUCTS)
    .where("provider_subscription_id", "<>", sub.providerSubscriptionId)
    .executeTakeFirst();
  return plan !== undefined;
}

const neverHeld = (rows: readonly Row[]) =>
  rows.every((row) => row.ended_at?.getTime() === row.started_at.getTime());

/**
 * A deleted account can still finish a checkout it opened before deleting. Its payer, the person
 * or the solo org it is the last member of, then gets the sweeper's turn to end the new plan.
 */
async function endingForDeletedPayer(trx: Kysely<Tables>, customerId: string) {
  const payer = trx
    .selectFrom("billing_customers")
    .where("id", "=", customerId);
  await trx
    .updateTable("users")
    .set({ billing_retry_at: sql<Date>`now()` })
    .where("deleted_at", "is not", null)
    .where("billing_retry_at", "is", null)
    .where((eb) =>
      eb.or([
        eb("id", "=", payer.select("user_id")),
        eb(
          "id",
          "in",
          eb
            .selectFrom("org_users")
            .select("user_id")
            .where("org_id", "=", payer.select("org_id")),
        ),
      ]),
    )
    .execute();
}

/** Applies one event and answers which customer it changed. */
async function apply(
  trx: Transaction<Tables>,
  provider: string,
  event: ProviderEvent,
): Promise<Applied> {
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
      if (!customer) throw new Gone("The customer");
      await endingForDeletedPayer(trx, customer.id);
      const sub = event.subscription;
      const surplus = await isSurplus(trx, customer.id, sub);
      await insertItems(trx, customer.id, sub, sub.items, at, { surplus });
      if (!surplus) return { customerId: customer.id };
      return {
        customerId: customer.id,
        afterwards: async (acting) => {
          for (const item of sub.items)
            await acting.cancel(
              {
                providerSubscriptionId: sub.providerSubscriptionId,
                providerCustomerId: event.providerCustomerId,
                product: item.product,
                interval: sub.interval,
                quantity: item.quantity,
                currentPeriodEnd: sub.currentPeriodEnd,
                cancelAtPeriodEnd: sub.cancelAtPeriodEnd,
              },
              "now",
            );
        },
      };
    }
    case "subscription.changed": {
      const sub = event.subscription;
      const { rows, customerId } = await rowsOf(
        trx,
        provider,
        sub.providerSubscriptionId,
        at,
      );
      if (isNewer(rows, at)) await changeItems(trx, rows, customerId, sub, at);
      return { customerId };
    }
    case "subscription.lapsed":
    case "subscription.ended": {
      const { rows, customerId } = await rowsOf(
        trx,
        provider,
        event.providerSubscriptionId,
        at,
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
      return { customerId };
    }
    case "payment.made":
    case "payment.failed": {
      const { rows, customerId } = await rowsOf(
        trx,
        provider,
        event.providerSubscriptionId,
        at,
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
      if (!paid || !neverHeld(rows)) return { customerId };
      const { providerPaymentId } = event;
      return {
        customerId,
        afterwards: (acting) => acting.refund(providerPaymentId),
      };
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
        return { customerId: invoice.customer_id };
      }
      const known = await trx
        .selectFrom("invoices")
        .select("customer_id")
        .where("provider_payment_id", "=", event.providerPaymentId)
        .executeTakeFirst();
      if (!known) throw unknown("The payment", at);
      return { customerId: known.customer_id };
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
 * Each goes in its own transaction with the plan.changed events for the people it concerns. An
 * event whose call to the provider then fails is forgotten, so the provider's retry repeats both.
 */
export async function applyProviderEvents(
  db: Kysely<Tables>,
  provider: Acting,
  events: readonly ProviderEvent[],
  log: Logger,
): Promise<void> {
  const name = provider.name;
  for (const event of events) {
    let applied: Applied | undefined;
    try {
      applied = await db.transaction().execute(async (trx) => {
        const fresh = await trx
          .insertInto("billing_events")
          .values({ provider: name, event_id: event.eventId, type: event.type })
          .onConflict((oc) => oc.columns(["provider", "event_id"]).doNothing())
          .returning("event_id")
          .executeTakeFirst();
        if (!fresh) return undefined;
        const result = await apply(trx, name, event);
        const people = await peopleOf(trx, result.customerId);
        await appendEvents(
          trx,
          people.map((userId) => ({ userId, type: "plan.changed" as const })),
        );
        log.info(
          { provider: name, type: event.type, customerId: result.customerId },
          "billing event",
        );
        return result;
      });
    } catch (error) {
      if (!(error instanceof Gone)) throw error;
      log.info(
        { provider: name, type: event.type, gone: error.message },
        "acknowledged a billing event about something erased",
      );
      continue;
    }
    if (!applied?.afterwards) continue;
    try {
      await applied.afterwards(provider);
    } catch (error) {
      await db
        .deleteFrom("billing_events")
        .where("provider", "=", name)
        .where("event_id", "=", event.eventId)
        .execute();
      log.error(
        { err: error, provider: name, type: event.type },
        "could not undo a second plan with the payment provider",
      );
      throw error;
    }
  }
}
