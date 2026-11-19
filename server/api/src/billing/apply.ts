import type { InvoiceStatus } from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import { ApiFailure } from "../http.ts";
import type { Logger } from "../log.ts";
import {
  callLater,
  makeDueCalls,
  type Calling,
  type ProviderCall,
} from "./calls.ts";
import { PLAN_PRODUCTS, product } from "./catalog.ts";
import { openerRole, refusal } from "./checks.ts";
import {
  heldSubscription,
  type ProviderEvent,
  type ProviderItem,
  type ProviderSubscription,
} from "./provider.ts";
import { refundPending } from "./refunds.ts";
import { checkSeatPauses, keptShare } from "./seat-pause.ts";
import { claimSettledBy } from "./seats.ts";

/** An event about something erased, which is acknowledged and changes nothing. */
class Gone extends Error {}

/** No payer, as for an event about a subscription that did not come from the app's checkout. */
const STRAY = { customerId: null };

/** A checkout the API opened with its own key and was paid, which it has no record of. */
class UnknownSale extends ApiFailure {
  readonly providerSubscriptionId: string;
  readonly providerCheckoutId: string;
  constructor(providerSubscriptionId: string, providerCheckoutId: string) {
    super(503, "unavailable", "This checkout is not known yet.");
    this.providerSubscriptionId = providerSubscriptionId;
    this.providerCheckoutId = providerCheckoutId;
  }
}

/** The provider an event came from, as applying it needs. */
interface From {
  name: string;
  eventRetrySpanMs: number;
}

/**
 * An event about something unknown is answered "not yet", for the provider to send again once
 * what it follows has arrived. Once the provider would have stopped sending it, what it is about
 * was erased with its account.
 */
function unknown(from: From, what: string, at: Date): Error {
  if (Date.now() - at.getTime() > from.eventRetrySpanMs) return new Gone(what);
  return new ApiFailure(503, "unavailable", `${what} is not known yet.`);
}

/** The rows of one provider subscription, one per product, locked for the event. */
async function rowsOf(
  trx: Transaction<Tables>,
  from: From,
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
      "subscriptions.paused_for_seat_at",
      "subscriptions.paid_left",
      "subscriptions.resumes_at",
      "subscriptions.cancel_at_period_end",
      "subscriptions.cancel_asked_at",
      "subscriptions.resume_asked_at",
      "billing_customers.provider_customer_id",
    ])
    .where("billing_customers.provider", "=", from.name)
    .where(
      "subscriptions.provider_subscription_id",
      "=",
      providerSubscriptionId,
    )
    .orderBy("subscriptions.started_at")
    .forUpdate("subscriptions")
    .execute();
  const [first] = rows;
  if (first) return { rows, customerId: first.customer_id };
  if (await isStray(trx, from, providerSubscriptionId)) return null;
  const sale = await trx
    .selectFrom("billing_held_sales")
    .select("provider_checkout_id")
    .where("provider", "=", from.name)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .executeTakeFirst();
  if (sale)
    throw new UnknownSale(providerSubscriptionId, sale.provider_checkout_id);
  throw unknown(from, "The subscription", at);
}

async function isStray(
  trx: Transaction<Tables>,
  from: From,
  providerId: string,
) {
  const stray = await trx
    .selectFrom("billing_strays")
    .select("provider_id")
    .where("provider", "=", from.name)
    .where("provider_id", "=", providerId)
    .executeTakeFirst();
  return Boolean(stray);
}

/** Remembers a subscription or payment that is not ours, so its later events are acknowledged. */
async function keepStrays(
  trx: Transaction<Tables>,
  from: From,
  providerIds: readonly string[],
) {
  await trx
    .insertInto("billing_strays")
    .values(providerIds.map((id) => ({ provider: from.name, provider_id: id })))
    .onConflict((oc) => oc.columns(["provider", "provider_id"]).doNothing())
    .execute();
}

/** A held subscription for each product of one the provider described, to cancel it by. */
function heldItems(
  sub: ProviderSubscription,
  providerCustomerId: string,
): ProviderCall[] {
  return sub.items.map((item) => ({
    action: "cancel_now",
    subscription: {
      providerSubscriptionId: sub.providerSubscriptionId,
      providerCustomerId,
      product: item.product,
      interval: sub.interval,
      quantity: item.quantity,
      currentPeriodEnd: sub.currentPeriodEnd,
      cancelAtPeriodEnd: sub.cancelAtPeriodEnd,
    },
  }));
}

type Row = NonNullable<Awaited<ReturnType<typeof rowsOf>>>["rows"][number];

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
  provider: string,
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
  for (const { item, row } of pairs) {
    if (!row || (row.status === "ended" && !live.length)) continue;
    const pausing =
      (row.paused_for_seat_at !== null &&
        row.paid_left === null &&
        row.resumes_at === null) ||
      cancelledWhilePausedForSeat(row);
    const back = {
      ...(row.status !== "active" && {
        status: "active" as const,
        ended_at: null,
      }),
      ...(!pausing && {
        paused_for_seat_at: null,
        paid_left: null,
        resumes_at: null,
      }),
    };
    const updated = await trx
      .updateTable("subscriptions")
      .set({
        ...itemColumns(sub, item, at),
        ...claimSettledBy(
          Math.min(item.quantity, item.atRenewal?.quantity ?? item.quantity),
          at,
        ),
        ...back,
        ...(row.cancel_asked_at && { cancel_at_period_end: true }),
      })
      .where("id", "=", row.id)
      .returningAll()
      .executeTakeFirstOrThrow();
    if (
      row.cancel_asked_at &&
      !sub.cancelAtPeriodEnd &&
      !pausing &&
      updated.status !== "ended"
    )
      await callLater(trx, provider, [
        {
          action: "keep_cancel",
          subscription: heldSubscription(updated, row.provider_customer_id),
        },
      ]);
  }
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

/** Whether a payment bought nothing: the plan had ended, or ended while this payment was failing. */
function paidForNothing(
  rows: readonly Row[],
  at: Date,
  before: InvoiceStatus | undefined,
) {
  if (!rows.every((row) => row.status === "ended")) return false;
  if (before === "failed" || neverHeld(rows)) return true;
  return rows.every((row) => row.ended_at && row.ended_at <= at);
}

function cancelledWhilePausedForSeat(row: Row) {
  return row.cancel_at_period_end && row.paused_for_seat_at !== null;
}

/**
 * A plan its person cancelled never renews, though the provider can still charge a renewal when it
 * lost the cancel or never took it. A renewal for a period begun after the cancel is refunded.
 */
function renewedAfterCancel(rows: readonly Row[], periodStart: Date) {
  return rows.some(
    (row) => row.cancel_asked_at !== null && row.cancel_asked_at < periodStart,
  );
}

/**
 * Whether a payment is the first charge after we asked the provider to resume a plan paused for a
 * seat. The provider may call it a change, but it pays for a new period as a renewal does.
 */
async function chargedOnResume(
  trx: Transaction<Tables>,
  rows: readonly Row[],
  event: Extract<ProviderEvent, { type: "payment.made" | "payment.failed" }>,
  periodStart: Date,
) {
  if (event.reason !== "change") return false;
  const asked = rows
    .map((row) => row.resume_asked_at)
    .filter((at): at is Date => at !== null && at <= periodStart);
  if (!asked.length) return false;
  const since = new Date(Math.max(...asked.map((at) => at.getTime())));
  const renewal = await trx
    .selectFrom("invoices")
    .select("id")
    .where("provider_subscription_id", "=", event.providerSubscriptionId)
    .where("provider_payment_id", "<>", event.providerPaymentId)
    .where("reason", "in", ["first_payment", "renewal"])
    .where("at", ">=", since)
    .executeTakeFirst();
  return !renewal;
}

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

/**
 * The customer a paid checkout of ours buys for, once what was paid passes the checks it passed
 * when opened, under the provider's customer it was opened for. Null for any other payment.
 */
async function paidCheckout(
  trx: Transaction<Tables>,
  from: From,
  event: Extract<ProviderEvent, { type: "subscription.started" }>,
) {
  const checkout = await trx
    .selectFrom("billing_checkouts")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "billing_checkouts.customer_id",
    )
    .select([
      "billing_checkouts.customer_id",
      "billing_checkouts.opened_by",
      "billing_checkouts.provider_customer_id",
      "billing_checkouts.product",
      "billing_checkouts.interval",
      "billing_customers.org_id",
    ])
    .where("billing_checkouts.provider", "=", from.name)
    .where(
      "billing_checkouts.provider_checkout_id",
      "=",
      event.providerCheckoutId,
    )
    .executeTakeFirst();
  if (
    !checkout &&
    !event.openedByBuyer &&
    !(await isStray(trx, from, event.providerCheckoutId))
  )
    throw new UnknownSale(
      event.subscription.providerSubscriptionId,
      event.providerCheckoutId,
    );
  const sub = event.subscription;
  const [item, ...more] = sub.items;
  if (
    !checkout ||
    !item ||
    more.length ||
    checkout.provider_customer_id !== event.providerCustomerId ||
    checkout.product !== item.product ||
    checkout.interval !== sub.interval
  )
    return null;
  const role = await openerRole(trx, checkout.org_id, checkout.opened_by);
  const customer = await trx
    .selectFrom("billing_customers")
    .select(["id", "paid_by"])
    .where("id", "=", checkout.customer_id)
    .forUpdate()
    .executeTakeFirstOrThrow();
  const refused = await refusal(trx, {
    customerId: customer.id,
    orgId: checkout.org_id,
    paidBy: customer.paid_by,
    openedBy: checkout.opened_by,
    openerRole: role,
    product: item.product,
    quantity: item.quantity,
  });
  if (refused) return null;
  await trx
    .updateTable("billing_customers")
    .set({
      provider_customer_id: checkout.provider_customer_id,
      ...(checkout.org_id === null ? {} : { paid_by: checkout.opened_by }),
    })
    .where("id", "=", customer.id)
    .execute();
  return customer;
}

/** Applies one event and answers which customer it changed. */
async function apply(
  trx: Transaction<Tables>,
  from: From,
  event: ProviderEvent,
): Promise<{
  customerId: string | null;
  cancelled?: string;
  refused?: { payment: string; invoice: string };
}> {
  const provider = from.name;
  const { at } = event;
  switch (event.type) {
    case "subscription.started": {
      const sub = event.subscription;
      const customer = await paidCheckout(trx, from, event);
      if (!customer) {
        const payment = event.providerPaymentId;
        await keepStrays(trx, from, [
          sub.providerSubscriptionId,
          ...(payment ? [payment] : []),
        ]);
        await callLater(trx, provider, [
          ...heldItems(sub, event.providerCustomerId),
          ...(payment
            ? [{ action: "refund" as const, providerPaymentId: payment }]
            : []),
        ]);
        return { ...STRAY, cancelled: sub.providerSubscriptionId };
      }
      await endingForDeletedPayer(trx, customer.id);
      const surplus = await isSurplus(trx, customer.id, sub);
      await insertItems(trx, customer.id, sub, sub.items, at, { surplus });
      if (surplus)
        await callLater(
          trx,
          provider,
          heldItems(sub, event.providerCustomerId),
        );
      return { customerId: customer.id };
    }
    case "subscription.changed": {
      const sub = event.subscription;
      const held = await rowsOf(trx, from, sub.providerSubscriptionId, at);
      if (!held) return STRAY;
      const { rows, customerId } = held;
      if (isNewer(rows, at))
        await changeItems(trx, provider, rows, customerId, sub, at);
      return { customerId };
    }
    case "subscription.paused": {
      const held = await rowsOf(trx, from, event.providerSubscriptionId, at);
      if (!held) return STRAY;
      const { rows, customerId } = held;
      const ids = rows
        .filter((row) => row.status !== "ended")
        .map((row) => row.id);
      if (ids.length && isNewer(rows, at))
        await trx
          .updateTable("subscriptions")
          .set({ status: "paused", provider_updated_at: at })
          .where("id", "in", ids)
          .execute();
      return { customerId };
    }
    case "subscription.lapsed":
    case "subscription.ended": {
      const held = await rowsOf(trx, from, event.providerSubscriptionId, at);
      if (!held) return STRAY;
      const { rows, customerId } = held;
      const ended = event.type === "subscription.ended";
      const ids = rows
        .filter((row) => row.status !== "ended")
        .map((row) => row.id);
      const cured =
        !ended &&
        (await trx
          .selectFrom("invoices")
          .select("id")
          .where("provider_subscription_id", "=", event.providerSubscriptionId)
          .where("status", "in", ["paid", "refunded"])
          .where("at", ">=", at)
          .executeTakeFirst());
      if (ids.length && !cured && (ended || isNewer(rows, at)))
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
      const paid = event.type === "payment.made";
      const held = await rowsOf(trx, from, event.providerSubscriptionId, at);
      if (!held) {
        if (paid && !(await isStray(trx, from, event.providerPaymentId))) {
          await keepStrays(trx, from, [event.providerPaymentId]);
          await callLater(trx, provider, [
            { action: "refund", providerPaymentId: event.providerPaymentId },
          ]);
        }
        return STRAY;
      }
      const { rows, customerId } = held;
      const before = await trx
        .selectFrom("invoices")
        .select("status")
        .where("provider_payment_id", "=", event.providerPaymentId)
        .forUpdate()
        .executeTakeFirst();
      const reason = (await chargedOnResume(
        trx,
        rows,
        event,
        event.periodStart ?? at,
      ))
        ? "renewal"
        : event.reason;
      await trx
        .insertInto("invoices")
        .values({
          customer_id: customerId,
          provider_subscription_id: event.providerSubscriptionId,
          provider_payment_id: event.providerPaymentId,
          lines: JSON.stringify(event.lines),
          reason,
          amount: event.total.amount,
          currency: event.total.currency,
          status: paid ? "paid" : "failed",
          at,
          refund_requested_at: null,
          refunded_at: null,
        })
        .onConflict((oc) =>
          paid
            ? oc
                .column("provider_payment_id")
                .doUpdateSet({
                  status: "paid",
                  at,
                  amount: event.total.amount,
                  currency: event.total.currency,
                })
                .where("invoices.status", "=", "failed")
            : oc.column("provider_payment_id").doNothing(),
        )
        .execute();
      const lapsed = rows
        .filter((row) => row.status === "lapsed")
        .map((row) => row.id);
      if (paid && lapsed.length && isNewer(rows, at)) {
        await trx
          .updateTable("subscriptions")
          .set({ status: "active" })
          .where("id", "in", lapsed)
          .execute();
        if (event.periodEnd)
          await trx
            .updateTable("subscriptions")
            .set({ current_period_end: event.periodEnd })
            .where("id", "in", lapsed)
            .where("current_period_end", "<", event.periodEnd)
            .execute();
      }
      const fresh = !before || before.status === "failed";
      if (paid && fresh && paidForNothing(rows, at, before?.status))
        await callLater(trx, provider, [
          { action: "refund", providerPaymentId: event.providerPaymentId },
        ]);
      else if (
        paid &&
        fresh &&
        reason === "renewal" &&
        renewedAfterCancel(rows, event.periodStart ?? at)
      ) {
        await callLater(trx, provider, [
          { action: "refund", providerPaymentId: event.providerPaymentId },
        ]);
        await endPlanNow(
          trx,
          from,
          customerId,
          event.providerSubscriptionId,
          at,
        );
      }
      return { customerId };
    }
    case "payment.refunded":
    case "payment.refund_refused": {
      const invoice = await trx
        .selectFrom("invoices")
        .selectAll()
        .where("provider_payment_id", "=", event.providerPaymentId)
        .forUpdate()
        .executeTakeFirst();
      if (!invoice) {
        if (await isStray(trx, from, event.providerPaymentId)) return STRAY;
        throw unknown(from, "The payment", at);
      }
      const customerId = invoice.customer_id;
      const subscriptionId = invoice.provider_subscription_id;
      if (invoice.status !== "paid") return { customerId };
      if (event.type === "payment.refunded") {
        const recorded = await trx
          .insertInto("invoice_refunds")
          .values({
            provider_refund_id: event.providerRefundId,
            invoice_id: invoice.id,
            amount: event.returned.amount,
          })
          .onConflict((oc) => oc.column("provider_refund_id").doNothing())
          .returning("provider_refund_id")
          .executeTakeFirst();
        if (!recorded) return { customerId };
      }
      const keptPending =
        invoice.kept_refund !== null && invoice.kept_refunded_at === null;
      if (event.type === "payment.refund_refused" && keptPending) {
        if (invoice.kept_refund_refused_at)
          return {
            customerId,
            refused: { payment: event.providerPaymentId, invoice: invoice.id },
          };
        await trx
          .updateTable("invoices")
          .set({ kept_refund_refused_at: at })
          .where("id", "=", invoice.id)
          .execute();
        await keptRefundsAnswered(trx, from, customerId, subscriptionId, at);
        return {
          customerId,
          refused: { payment: event.providerPaymentId, invoice: invoice.id },
        };
      }
      if (event.type === "payment.refund_refused") {
        await trx
          .updateTable("invoices")
          .set({ refund_requested_at: null })
          .where("customer_id", "=", customerId)
          .where("provider_subscription_id", "=", subscriptionId)
          .where("status", "=", "paid")
          .execute();
        return {
          customerId,
          refused: { payment: event.providerPaymentId, invoice: invoice.id },
        };
      }
      if (keptPending && !event.chargeback) {
        const whole = event.returned.amount >= Number(invoice.amount);
        await trx
          .updateTable("invoices")
          .set({
            kept_refunded_at: at,
            ...(whole && { status: "refunded" as const, refunded_at: at }),
          })
          .where("id", "=", invoice.id)
          .execute();
        if (invoice.kept_refund_refused_at)
          await endPlanNow(trx, from, customerId, subscriptionId, at);
        else
          await keptRefundsAnswered(trx, from, customerId, subscriptionId, at);
        return { customerId };
      }
      if (event.chargeback) {
        await trx
          .updateTable("invoices")
          .set({ status: "disputed" })
          .where("id", "=", invoice.id)
          .execute();
        await endPlanNow(trx, from, customerId, subscriptionId, at);
        return { customerId };
      }
      const whole = event.returned.amount >= Number(invoice.amount);
      if (whole)
        await trx
          .updateTable("invoices")
          .set({ status: "refunded", refunded_at: at })
          .where("id", "=", invoice.id)
          .execute();
      if (
        (await keptTimeRefunded(trx, customerId, subscriptionId, invoice.id)) ||
        (whole &&
          invoice.refund_requested_at &&
          (await refundGiven(trx, customerId, subscriptionId)))
      )
        await endPlanNow(trx, from, customerId, subscriptionId, at);
      return { customerId };
    }
  }
}

/**
 * Once the provider has answered every refund of a paused plan's kept time, ends the plan if it paid
 * them all. Otherwise the person keeps the time whose refund was refused: the kept time is cut by
 * the share of it paid back, by amount, and the plan runs the rest.
 */
async function keptRefundsAnswered(
  trx: Transaction<Tables>,
  from: From,
  customerId: string,
  providerSubscriptionId: string,
  at: Date,
) {
  const kept = await trx
    .selectFrom("invoices")
    .select(["kept_refund", "kept_refunded_at", "kept_refund_refused_at"])
    .where("customer_id", "=", customerId)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .where("kept_refund", "is not", null)
    .execute();
  if (
    kept.some(
      (invoice) =>
        invoice.kept_refunded_at === null &&
        invoice.kept_refund_refused_at === null,
    )
  )
    return;
  let total = 0;
  let paidBack = 0;
  for (const invoice of kept) {
    const amount = Number(invoice.kept_refund);
    total += amount;
    if (invoice.kept_refunded_at) paidBack += amount;
  }
  if (paidBack >= total) {
    await endPlanNow(trx, from, customerId, providerSubscriptionId, at);
    return;
  }
  if (paidBack === 0) return;
  await trx
    .updateTable("subscriptions")
    .set({
      paid_left: sql<string>`paid_left * ${(total - paidBack) / total}::float8`,
    })
    .where("customer_id", "=", customerId)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .where("paid_left", "is not", null)
    .execute();
}

/** Whether a refund asked for on a plan has been paid in full, its first payment included. */
async function refundGiven(
  trx: Transaction<Tables>,
  customerId: string,
  providerSubscriptionId: string,
) {
  const first = await trx
    .selectFrom("invoices")
    .select(["status", "refund_requested_at"])
    .where("customer_id", "=", customerId)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .where("reason", "=", "first_payment")
    .executeTakeFirst();
  return (
    first?.status === "refunded" &&
    first.refund_requested_at !== null &&
    !(await refundPending(trx, customerId, providerSubscriptionId))
  );
}

/**
 * Whether staff have refunded by hand the kept time of a cancelled plan whose payment never came in
 * time to refund it: the refunds of its current period's payments reach what the kept time would
 * have refunded, short of a day's worth. Less is goodwill, and the plan keeps its time.
 */
async function keptTimeRefunded(
  trx: Transaction<Tables>,
  customerId: string,
  providerSubscriptionId: string,
  invoiceId: string,
) {
  const kept = await trx
    .selectFrom("subscriptions")
    .select([
      "current_period_end",
      sql<Date>`current_period_end - case interval when 'year' then interval '1 year' else interval '1 month' end - interval '3 days'`.as(
        "period_started_before",
      ),
      sql<number>`extract(epoch from coalesce(paid_left, greatest(resumes_at - now(), interval '0')))::float8 - 86400`.as(
        "kept_seconds",
      ),
    ])
    .where("customer_id", "=", customerId)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .where("status", "<>", "ended")
    .where("kept_without_refund_at", "is not", null)
    .where("cancel_asked_at", "is not", null)
    .executeTakeFirst();
  if (!kept) return false;
  const ofPlan = trx
    .selectFrom("invoices")
    .where("customer_id", "=", customerId)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .where("status", "in", ["paid", "refunded"]);
  const started = await ofPlan
    .select("at")
    .where("reason", "in", ["first_payment", "renewal"])
    .where("at", ">=", kept.period_started_before)
    .orderBy("at", "desc")
    .limit(1)
    .executeTakeFirst();
  if (!started) return false;
  const period = await ofPlan
    .select([
      "invoices.id",
      "invoices.amount",
      "invoices.at",
      (eb) =>
        eb
          .selectFrom("invoice_refunds")
          .select(sql<string>`coalesce(sum(amount), 0)`.as("refunded"))
          .whereRef("invoice_refunds.invoice_id", "=", "invoices.id")
          .as("refunded"),
    ])
    .where("at", ">=", started.at)
    .execute();
  if (!period.some((invoice) => invoice.id === invoiceId)) return false;
  let owed = 0;
  let refunded = 0;
  for (const invoice of period) {
    owed += Math.max(
      0,
      keptShare(invoice, kept.current_period_end, kept.kept_seconds),
    );
    refunded += Number(invoice.refunded);
  }
  return refunded >= owed;
}

/** A plan refunded, or whose payment the bank took back, ends now, here and with the provider. */
async function endPlanNow(
  trx: Transaction<Tables>,
  from: From,
  customerId: string,
  providerSubscriptionId: string,
  at: Date,
) {
  const live = await trx
    .updateTable("subscriptions")
    .set({ status: "ended", ended_at: at, provider_updated_at: at })
    .where("customer_id", "=", customerId)
    .where("provider_subscription_id", "=", providerSubscriptionId)
    .where("status", "<>", "ended")
    .returningAll()
    .execute();
  if (!live.length) return;
  const customer = await trx
    .selectFrom("billing_customers")
    .select("provider_customer_id")
    .where("id", "=", customerId)
    .executeTakeFirstOrThrow();
  await callLater(
    trx,
    from.name,
    live.map((row) => ({
      action: "cancel_now",
      subscription: heldSubscription(row, customer.provider_customer_id),
    })),
  );
}

/** The people whose plan a customer's subscriptions decide: the person, or the org's members. */
export async function peopleOf(
  db: Kysely<Tables>,
  customerId: string,
): Promise<{ people: string[]; org: boolean }> {
  const customer = await db
    .selectFrom("billing_customers")
    .select(["user_id", "org_id"])
    .where("id", "=", customerId)
    .executeTakeFirstOrThrow();
  if (customer.user_id) return { people: [customer.user_id], org: false };
  if (!customer.org_id) return { people: [], org: false };
  const members = await db
    .selectFrom("org_users")
    .select("user_id")
    .where("org_id", "=", customer.org_id)
    .execute();
  return { people: members.map((member) => member.user_id), org: true };
}

/**
 * Applies a provider's events, each once: one already handled, even before a restart, is skipped.
 * Each goes in its own transaction with the plan.changed events for the people it concerns and the
 * calls to the provider it leads to, which are then made.
 */
export async function applyProviderEvents(
  db: Kysely<Tables>,
  provider: Calling,
  events: readonly ProviderEvent[],
  log: Logger,
): Promise<void> {
  try {
    for (const event of events) await applyOnce(db, provider, event, log);
  } finally {
    await makeDueCalls(db, provider, log);
  }
}

async function applyOnce(
  db: Kysely<Tables>,
  from: From,
  event: ProviderEvent,
  log: Logger,
) {
  const { name } = from;
  try {
    await db.transaction().execute(async (trx) => {
      const fresh = await trx
        .insertInto("billing_events")
        .values({ provider: name, event_id: event.eventId, type: event.type })
        .onConflict((oc) => oc.columns(["provider", "event_id"]).doNothing())
        .returning("event_id")
        .executeTakeFirst();
      if (!fresh) return;
      if (
        event.type === "payment.made" &&
        event.reason === "renewal" &&
        !event.periodStart
      )
        log.error(
          {
            provider: name,
            payment: event.providerPaymentId,
            fingerprint: [
              "renewal_without_period",
              event.providerSubscriptionId,
            ],
          },
          "a renewal came without its billing period, so whether it follows a cancel is judged by when it was paid",
        );
      const result = await apply(trx, from, event);
      const { people, org } = result.customerId
        ? await peopleOf(trx, result.customerId)
        : { people: [], org: false };
      await appendEvents(
        trx,
        people.flatMap((userId) => [
          { userId, type: "plan.changed" as const },
          ...(org ? [{ userId, type: "orgs.changed" as const }] : []),
        ]),
      );
      await checkSeatPauses(trx, people);
      if (result.refused)
        log.error(
          {
            provider: name,
            payment: result.refused.payment,
            fingerprint: ["refund_refused", result.refused.invoice],
          },
          "the payment provider refused a refund: the plan goes on with its paid time, and the person is owed an answer",
        );
      if (result.cancelled)
        log.warn(
          { provider: name, subscription: result.cancelled },
          "cancelling and refunding a subscription paid outside the app's checkout",
        );
      log.info(
        { provider: name, type: event.type, customerId: result.customerId },
        "billing event",
      );
    });
  } catch (error) {
    if (error instanceof UnknownSale) {
      await db
        .insertInto("billing_held_sales")
        .values({
          provider: name,
          provider_subscription_id: error.providerSubscriptionId,
          provider_checkout_id: error.providerCheckoutId,
        })
        .onConflict((oc) =>
          oc.columns(["provider", "provider_subscription_id"]).doNothing(),
        )
        .execute();
      log.error(
        {
          provider: name,
          type: event.type,
          checkout: error.providerCheckoutId,
          subscription: error.providerSubscriptionId,
        },
        "a checkout the API opened was paid but has no record here: its subscription and payment are left alone, and its events refused until someone looks",
      );
    }
    if (!(error instanceof Gone)) throw error;
    log.info(
      { provider: name, type: event.type, gone: error.message },
      "acknowledged a billing event about something erased",
    );
  }
}
