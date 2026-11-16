import { createHmac, timingSafeEqual } from "node:crypto";
import type {
  BillingInterval,
  BillingProductId,
  ChangeWhen,
  InvoiceLine,
  InvoiceReason,
  Money,
  PaymentCard,
} from "@sikemux/protocol";

import { ApiFailure } from "../http.ts";
import type { Logger } from "../log.ts";
import {
  CLIENT_TOKEN_NAME,
  type PaddleApi,
  PaddleError,
  priceTag,
  WEBHOOK_PATH,
  type PaddlePrice,
} from "./paddle-api.ts";
import type {
  BillingProvider,
  ChangeOutcome,
  ChangePreview,
  CheckoutRequest,
  HeldSubscription,
  ProviderEvent,
  ProviderItem,
  ProviderRequest,
  ProviderSubscription,
  SubscriptionUpdate,
} from "./provider.ts";

/** How far a webhook's signed time may be from ours. Sending one again is harmless, so it is generous. */
const SIGNATURE_TOLERANCE_S = 5 * 60;
const REFETCH_AFTER_MS = 60_000;

/** A change waiting for renewal: Paddle already holds the new items, so this is what is held until then. */
interface Held {
  product: BillingProductId;
  quantity: number;
  interval: BillingInterval;
  until: string;
}

interface CustomData {
  /** The subscription this was written for. */
  subscription?: string;
  held?: Held;
  /** Where a change taken back leaves the next bill: the end of the period paid for. */
  bill_on?: string;
}

interface PaddleItem {
  status: string;
  quantity: number;
  price: PaddlePrice;
}

interface PaddleSubscription {
  id: string;
  status: "active" | "canceled" | "past_due" | "paused" | "trialing";
  customer_id: string;
  custom_data: CustomData | null;
  billing_cycle: { interval: string };
  current_billing_period: { starts_at: string; ends_at: string } | null;
  next_billed_at: string | null;
  scheduled_change: { action: string; effective_at: string } | null;
  items: PaddleItem[];
  management_urls?: { update_payment_method: string | null } | null;
}

interface PaddleTransaction {
  id: string;
  origin: string;
  customer_id: string | null;
  subscription_id: string | null;
  currency_code: string;
  billing_period: { starts_at: string; ends_at: string } | null;
  items: { quantity: number; price: PaddlePrice }[];
  details: { totals: { grand_total: string } };
  payments: {
    status: string;
    method_details: {
      type: string;
      card: {
        type: string;
        last4: string;
        expiry_month: number;
        expiry_year: number;
      } | null;
    } | null;
  }[];
  checkout: { url: string | null } | null;
}

interface PreviewTotals {
  grand_total: string;
  currency_code: string;
}

interface PaddlePreview {
  currency_code: string;
  immediate_transaction: { details: { totals: PreviewTotals } } | null;
  next_transaction: {
    billing_period: { starts_at: string };
    details: { totals: PreviewTotals };
  } | null;
}

/** What the card is charged: after credit from the customer's balance, tax included. */
function moneyOf(totals: PreviewTotals): Money {
  return {
    amount: Number(totals.grand_total),
    currency: totals.currency_code.toLowerCase(),
  };
}

interface PaddleAdjustment {
  action: string;
  status: string;
  transaction_id: string;
  currency_code: string;
  totals: { total: string };
}

interface PaddleEvent {
  event_id: string;
  event_type: string;
  occurred_at: string;
  data: unknown;
}

const ALREADY_CANCELLED = new Set([
  "subscription_is_canceled_action_invalid",
  "subscription_update_when_canceled",
]);
const LOCKED_FOR_RENEWAL = new Set([
  "subscription_locked_renewal",
  "subscription_next_billed_at_too_soon",
]);
const ALREADY_REFUNDED = new Set([
  "adjustment_pending_refund_request",
  "adjustment_transaction_item_has_already_been_fully_adjusted",
]);

function intervalOf(cycle: { interval: string } | null): BillingInterval {
  const interval = cycle?.interval;
  if (interval !== "month" && interval !== "year")
    throw new Error(`Paddle billed every ${interval ?? "never"}.`);
  return interval;
}

/** The items on a price the setup made; one added in Paddle's dashboard is not ours to read. */
function ours<T extends { price: PaddlePrice }>(
  items: readonly T[],
): (T & { product: BillingProductId })[] {
  return items.flatMap((item) => {
    const tag = priceTag(item.price);
    return tag ? [{ ...item, product: tag.product }] : [];
  });
}

function linesOf(
  items: readonly { quantity: number; price: PaddlePrice }[],
): InvoiceLine[] {
  return (
    ours(items)
      // A change lists what it replaced with a negative quantity.
      .filter((item) => item.quantity > 0)
      .map(({ product, quantity }) => ({ product, quantity }))
  );
}

function reasonOf(origin: string): InvoiceReason | null {
  switch (origin) {
    case "web":
    case "api":
      return "first_payment";
    case "subscription_recurring":
      return "renewal";
    case "subscription_update":
    case "subscription_charge":
      return "change";
    default:
      return null;
  }
}

function periodEndOf(sub: PaddleSubscription): string {
  const ends =
    sub.current_billing_period?.ends_at ??
    sub.next_billed_at ??
    sub.scheduled_change?.effective_at;
  if (!ends) throw new Error(`Paddle's subscription ${sub.id} has no period.`);
  return ends;
}

/**
 * What the API wrote in a subscription's custom data. A buyer can set custom data on the checkout,
 * and the subscription copies it, but its id does not exist until the checkout is paid.
 */
function ownDataOf(sub: PaddleSubscription): CustomData {
  return sub.custom_data?.subscription === sub.id ? sub.custom_data : {};
}

/** A change waiting for renewal that Paddle already holds. */
function heldOf(sub: PaddleSubscription, at: Date) {
  const { held } = ownDataOf(sub);
  return held && new Date(held.until) > at ? held : null;
}

/**
 * The day the next bill is owed, while it is ahead. Changing the interval restarts Paddle's period
 * at the change, so the paid period's end rides in custom data until Paddle bills on it again.
 */
function owedOf(sub: PaddleSubscription, at: Date): string | null {
  const own = ownDataOf(sub);
  const owed = own.held?.until ?? own.bill_on;
  return owed && new Date(owed) > at ? owed : null;
}

function billedOnOwedDate(sub: PaddleSubscription, at: Date) {
  const owed = owedOf(sub, at);
  const ends = sub.next_billed_at ?? sub.current_billing_period?.ends_at;
  return (
    !owed ||
    sub.status !== "active" ||
    (!!ends && new Date(ends).getTime() === new Date(owed).getTime())
  );
}

function subscriptionOf(
  sub: PaddleSubscription,
  at: Date,
): ProviderSubscription | null {
  const items: ProviderItem[] = ours(
    sub.items.filter((item) => item.status !== "inactive"),
  ).map(({ product, quantity }) => ({ product, quantity }));
  if (!items.length) return null;
  const interval = intervalOf(sub.billing_cycle);
  const reported: ProviderSubscription = {
    providerSubscriptionId: sub.id,
    items,
    interval,
    currentPeriodEnd: new Date(owedOf(sub, at) ?? periodEndOf(sub)),
    cancelAtPeriodEnd: sub.scheduled_change?.action === "cancel",
  };
  const held = heldOf(sub, at);
  const [now] = items;
  if (!held || !now || items.length !== 1) return reported;
  return {
    ...reported,
    items: [
      {
        product: held.product,
        quantity: held.quantity,
        atRenewal: { product: now.product, quantity: now.quantity, interval },
      },
    ],
    interval: held.interval,
    currentPeriodEnd: new Date(held.until),
  };
}

/** The signature Paddle puts on a webhook: `ts=<seconds>;h1=<hex>`, with an h1 for each secret in use. */
function signedBy(header: string, body: string, secret: string, nowMs: number) {
  const fields = header.split(";").map((field) => field.split("="));
  const ts = fields.find(([name]) => name === "ts")?.[1];
  const signatures = fields
    .filter(([name]) => name === "h1")
    .map(([, value]) => value ?? "");
  if (!ts || !/^\d+$/.test(ts)) return false;
  if (Math.abs(nowMs / 1000 - Number(ts)) > SIGNATURE_TOLERANCE_S) return false;
  const expected = createHmac("sha256", secret)
    .update(`${ts}:${body}`)
    .digest();
  return signatures.some((signature) => {
    const given = Buffer.from(signature, "hex");
    return given.length === expected.length && timingSafeEqual(given, expected);
  });
}

export interface PaddleOptions {
  api: PaddleApi;
  apiOrigin: string;
  log: Logger;
  /** The webhook's signing secret, when known without asking Paddle. */
  webhookSecret?: string;
  now?: () => number;
}

/** Paddle's error with its message swapped out, since Paddle may quote the email it was sent. */
function withoutEmail(err: unknown, message: string) {
  return err instanceof PaddleError
    ? new PaddleError(err.status, err.code, message)
    : err;
}

/** A value read from Paddle, or none, and when it was read. */
interface Read {
  value: string | null;
  at: number;
}

/**
 * Paddle locks a subscription in the half hour before it renews. A change of interval takes two
 * calls, so it stops a little earlier, before the second call could land inside the lock.
 */
const INTERVAL_CHANGE_STOPS_MS = 35 * 60_000;

function renewsShortly() {
  return new ApiFailure(
    409,
    "conflict",
    "This subscription renews in the next few minutes. Change it after it renews.",
  );
}

function notReady() {
  return new ApiFailure(
    503,
    "unavailable",
    "The payment provider is not set up yet.",
  );
}

/**
 * Paddle Billing. Each checkout makes one Paddle subscription holding one product. Prices are the
 * ones the setup command made, found by what it wrote on them; the webhook secret and checkout
 * token are read from Paddle by the destination and name the setup gave them.
 */
export class PaddleProvider implements BillingProvider {
  readonly name = "paddle";
  private readonly api: PaddleApi;
  private readonly log: Logger;
  private readonly now: () => number;
  private readonly webhookUrl: string;
  private secret: Read | null;
  private prices: { all: PaddlePrice[]; at: number } | null = null;
  private clientToken: Read | null = null;

  constructor(options: PaddleOptions) {
    this.api = options.api;
    this.log = options.log;
    this.now = options.now ?? Date.now;
    this.webhookUrl = options.apiOrigin + WEBHOOK_PATH;
    this.secret = options.webhookSecret
      ? { value: options.webhookSecret, at: Infinity }
      : null;
  }

  get sandbox() {
    return this.api.sandbox;
  }

  /** Paddle retries for about two days live and for about two minutes in the sandbox. */
  get eventRetrySpanMs() {
    return this.sandbox ? 10 * 60_000 : 3 * 24 * 60 * 60_000;
  }

  private missing(what: string) {
    this.log.error(`Paddle has no ${what}: run the paddle-setup command`);
  }

  private notSetUp(what: string): never {
    this.missing(what);
    throw notReady();
  }

  /** What was read from Paddle, missing or not, stands for a minute; a stranger can't spend the key. */
  private async readAtMostEachMinute(
    known: Read | null,
    what: string,
    read: () => Promise<string | null>,
  ): Promise<Read> {
    if (known && this.now() - known.at < REFETCH_AFTER_MS) return known;
    const value = await read();
    if (!value) this.missing(what);
    return { value, at: this.now() };
  }

  private async priceFor(
    product: BillingProductId,
    interval: BillingInterval,
    unit: Money,
  ): Promise<PaddlePrice> {
    const find = () =>
      this.prices?.all.find((price) => {
        const tag = priceTag(price);
        return (
          tag?.product === product &&
          tag.interval === interval &&
          Number(price.unit_price.amount) === unit.amount &&
          price.unit_price.currency_code.toLowerCase() === unit.currency
        );
      });
    const cached = find();
    if (cached) return cached;
    if (!this.prices || this.now() - this.prices.at > REFETCH_AFTER_MS)
      this.prices = {
        all: await this.api.list<PaddlePrice>(
          "/prices?status=active&per_page=200",
        ),
        at: this.now(),
      };
    return find() ?? this.notSetUp(`price for ${product} each ${interval}`);
  }

  /** Read again on a bad signature, at most once a minute, in case the secret was rotated. */
  private async webhookSecret(again = false): Promise<string | null> {
    if (this.secret?.value && !again) return this.secret.value;
    this.secret = await this.readAtMostEachMinute(
      this.secret,
      "webhook destination",
      async () => {
        const settings = await this.api.list<{
          destination: string;
          active: boolean;
          endpoint_secret_key: string;
        }>("/notification-settings");
        return (
          settings.find(
            (setting) =>
              setting.destination === this.webhookUrl && setting.active,
          )?.endpoint_secret_key ?? null
        );
      },
    );
    return this.secret.value;
  }

  /** What the checkout page opens Paddle's checkout with. The token is public by Paddle's design. */
  async checkoutPage(): Promise<{ clientToken: string; sandbox: boolean }> {
    if (!this.clientToken?.value)
      this.clientToken = await this.readAtMostEachMinute(
        this.clientToken,
        "client token",
        async () => {
          const tokens = await this.api.list<{
            name: string;
            token: string;
          }>("/client-tokens?status=active");
          return (
            tokens.find((token) => token.name === CLIENT_TOKEN_NAME)?.token ??
            null
          );
        },
      );
    const clientToken = this.clientToken.value;
    if (!clientToken) throw notReady();
    return { clientToken, sandbox: this.sandbox };
  }

  async customerFor(_userId: string, email: string | null) {
    if (!email) return null;
    try {
      const made = await this.api.request<{ id: string }>(
        "POST",
        "/customers",
        { email },
      );
      return { id: made.id, made: true };
    } catch (err) {
      const existing =
        err instanceof PaddleError && err.code === "customer_already_exists"
          ? /ctm_[a-z0-9]+/.exec(err.message)?.[0]
          : undefined;
      if (!existing) throw withoutEmail(err, "no customer was made");
      await this.api.request("PATCH", `/customers/${existing}`, {
        status: "active",
      });
      return { id: existing, made: false };
    }
  }

  async readdress(providerCustomerId: string, email: string) {
    try {
      await this.api.request("PATCH", `/customers/${providerCustomerId}`, {
        email,
        status: "active",
      });
    } catch (err) {
      throw withoutEmail(err, "the customer's email was not moved");
    }
  }

  async checkout(request: CheckoutRequest) {
    const price = await this.priceFor(
      request.product,
      request.interval,
      request.unit,
    );
    const transaction = await this.api.request<PaddleTransaction>(
      "POST",
      "/transactions",
      {
        items: [{ price_id: price.id, quantity: request.quantity }],
        customer_id: request.providerCustomerId,
      },
    );
    if (!transaction.checkout?.url)
      throw new Error("Paddle made a checkout with no page.");
    return {
      url: transaction.checkout.url,
      providerCheckoutId: transaction.id,
    };
  }

  private subscription(id: string) {
    return this.api.request<PaddleSubscription>("GET", `/subscriptions/${id}`);
  }

  /** What the subscription's latest completed transaction was paid with. */
  private async lastPayment(subscriptionId: string) {
    const [last] = await this.api.request<PaddleTransaction[]>(
      "GET",
      `/transactions?subscription_id=${subscriptionId}&status=completed&order_by=created_at[DESC]&per_page=1`,
    );
    return last?.payments.find((payment) => payment.status === "captured");
  }

  /** UPI takes payments only on the dates its mandate names, so nothing can be charged mid-period. */
  private async paysByUpi(subscriptionId: string) {
    return (
      (await this.lastPayment(subscriptionId))?.method_details?.type === "upi"
    );
  }

  /** The update `change` sends Paddle for this input. */
  private async updateFor(
    held: HeldSubscription,
    update: SubscriptionUpdate,
    when: ChangeWhen,
  ) {
    const price = await this.priceFor(
      update.product,
      update.interval,
      update.unit,
    );
    const sub = await this.subscription(held.providerSubscriptionId);
    const items = [{ price_id: price.id, quantity: update.quantity }];
    if (when === "renewal") {
      const until = owedOf(sub, new Date(this.now())) ?? periodEndOf(sub);
      this.refuseLateIntervalChange(sub, update.interval, until);
      return {
        sub,
        until,
        body: {
          items,
          proration_billing_mode: "do_not_bill",
          custom_data: {
            subscription: sub.id,
            held: {
              product: held.product,
              quantity: held.quantity,
              interval: held.interval,
              until,
            } satisfies Held,
          },
        },
      };
    }
    const at = new Date(this.now());
    return {
      sub,
      refused: heldOf(sub, at)
        ? ("held" as const)
        : this.swapsAfterTakeBack(sub, price.id, at)
          ? ("swap_taken_back" as const)
          : null,
      body: {
        items,
        proration_billing_mode: "prorated_immediately",
        on_payment_failure: "prevent_change",
        custom_data: owedOf(sub, at)
          ? { subscription: sub.id, bill_on: owedOf(sub, at) }
          : {},
      },
    };
  }

  /** Paddle gives no credit for what was paid on a price it swapped away and back without billing. */
  private swapsAfterTakeBack(
    sub: PaddleSubscription,
    priceId: string,
    at: Date,
  ) {
    const { bill_on } = ownDataOf(sub);
    return (
      !!bill_on &&
      new Date(bill_on) > at &&
      !sub.items.some(
        (item) => item.status !== "inactive" && item.price.id === priceId,
      )
    );
  }

  private refuseLateIntervalChange(
    sub: PaddleSubscription,
    interval: BillingInterval,
    until: string,
  ) {
    if (
      interval !== intervalOf(sub.billing_cycle) &&
      new Date(until).getTime() - this.now() < INTERVAL_CHANGE_STOPS_MS
    )
      throw renewsShortly();
  }

  /** Paddle's lock before a renewal reaches the person as the refusal it is, not a server error. */
  private async alter<T>(method: string, path: string, body: unknown) {
    try {
      return await this.api.request<T>(method, path, body);
    } catch (err) {
      if (
        err instanceof PaddleError &&
        err.code &&
        LOCKED_FOR_RENEWAL.has(err.code)
      )
        throw renewsShortly();
      throw err;
    }
  }

  /** Paddle refuses items and a bill date in one request, so the date is put right after. */
  private async billOnOwedDate(subscriptionId: string) {
    const sub = await this.subscription(subscriptionId);
    const at = new Date(this.now());
    if (billedOnOwedDate(sub, at)) return;
    await this.alter("PATCH", `/subscriptions/${sub.id}`, {
      proration_billing_mode: "do_not_bill",
      next_billed_at: owedOf(sub, at),
    });
  }

  async takeBack(held: HeldSubscription, paid: SubscriptionUpdate) {
    const price = await this.priceFor(paid.product, paid.interval, paid.unit);
    const sub = await this.subscription(held.providerSubscriptionId);
    const at = new Date(this.now());
    if (!heldOf(sub, at)) return;
    const { held: _, bill_on: __, ...custom } = sub.custom_data ?? {};
    const billOn = owedOf(sub, at) ?? periodEndOf(sub);
    this.refuseLateIntervalChange(sub, paid.interval, billOn);
    await this.alter("PATCH", `/subscriptions/${sub.id}`, {
      items: [{ price_id: price.id, quantity: paid.quantity }],
      proration_billing_mode: "do_not_bill",
      custom_data: { ...custom, subscription: sub.id, bill_on: billOn },
    });
    await this.billOnOwedDate(sub.id);
  }

  async change(
    held: HeldSubscription,
    update: SubscriptionUpdate,
    when: ChangeWhen,
  ): Promise<ChangeOutcome> {
    const { sub, body, refused } = await this.updateFor(held, update, when);
    if (refused) return { taken: false, why: refused };
    if (when === "now" && (await this.paysByUpi(sub.id)))
      return { taken: false, why: "needs_renewal" };
    try {
      await this.alter("PATCH", `/subscriptions/${sub.id}`, body);
    } catch (err) {
      if (
        err instanceof PaddleError &&
        err.code === "subscription_payment_declined"
      )
        return { taken: false, why: "payment_failed" };
      throw err;
    }
    if (when === "renewal") await this.billOnOwedDate(sub.id);
    return { taken: true };
  }

  async previewChange(
    held: HeldSubscription,
    update: SubscriptionUpdate,
    when: ChangeWhen,
  ): Promise<ChangePreview> {
    const { sub, body, until, refused } = await this.updateFor(
      held,
      update,
      when,
    );
    if (refused) return { refused };
    if (when === "now" && (await this.paysByUpi(sub.id)))
      return { refused: "needs_renewal" };
    const preview = await this.alter<PaddlePreview>(
      "PATCH",
      `/subscriptions/${sub.id}/preview`,
      body,
    );
    const next = preview.next_transaction;
    return {
      now: moneyOf(
        preview.immediate_transaction?.details.totals ?? {
          grand_total: "0",
          currency_code: preview.currency_code,
        },
      ),
      renewal: next
        ? {
            at: new Date(until ?? next.billing_period.starts_at),
            amount: moneyOf(next.details.totals),
          }
        : null,
    };
  }

  async cancel(held: HeldSubscription, when: "period_end" | "now") {
    try {
      await this.alter(
        "POST",
        `/subscriptions/${held.providerSubscriptionId}/cancel`,
        {
          effective_from:
            when === "now" ? "immediately" : "next_billing_period",
        },
      );
    } catch (err) {
      if (
        err instanceof PaddleError &&
        err.code &&
        ALREADY_CANCELLED.has(err.code)
      )
        return;
      throw err;
    }
  }

  async resume(held: HeldSubscription) {
    await this.alter("PATCH", `/subscriptions/${held.providerSubscriptionId}`, {
      scheduled_change: null,
    });
  }

  async refund(providerPaymentId: string) {
    try {
      await this.api.request("POST", "/adjustments", {
        action: "refund",
        type: "full",
        transaction_id: providerPaymentId,
        reason: "A first payment refunded on request within the refund window.",
      });
    } catch (err) {
      if (
        err instanceof PaddleError &&
        err.code &&
        ALREADY_REFUNDED.has(err.code)
      )
        return;
      throw err;
    }
  }

  async portal(held: HeldSubscription) {
    const sub = await this.subscription(held.providerSubscriptionId);
    if (!held.providerCustomerId || sub.customer_id !== held.providerCustomerId)
      return null;
    const url = sub.management_urls?.update_payment_method;
    return url ? { url } : null;
  }

  async receipt(providerPaymentId: string) {
    try {
      const invoice = await this.api.request<{ url: string }>(
        "GET",
        `/transactions/${providerPaymentId}/invoice?disposition=inline`,
      );
      return { url: invoice.url };
    } catch (err) {
      if (
        err instanceof PaddleError &&
        err.status >= 400 &&
        err.status < 500 &&
        err.status !== 401 &&
        err.status !== 403
      )
        return null;
      throw err;
    }
  }

  async card(held: HeldSubscription): Promise<PaymentCard | null> {
    const card = (await this.lastPayment(held.providerSubscriptionId))
      ?.method_details?.card;
    return card
      ? {
          brand: card.type,
          last4: card.last4,
          expMonth: card.expiry_month,
          expYear: card.expiry_year,
        }
      : null;
  }

  async readEvents(request: ProviderRequest): Promise<ProviderEvent[] | null> {
    const header = request.header("paddle-signature");
    if (!header) return null;
    const secret = await this.webhookSecret();
    if (!secret) throw notReady();
    if (!signedBy(header, request.body, secret, this.now())) {
      const fresh = await this.webhookSecret(true);
      if (
        !fresh ||
        fresh === secret ||
        !signedBy(header, request.body, fresh, this.now())
      )
        return null;
    }
    const event = JSON.parse(request.body) as PaddleEvent;
    if (
      event.event_type.startsWith("subscription.") &&
      !billedOnOwedDate(event.data as PaddleSubscription, new Date(this.now()))
    )
      await this.billOnOwedDate((event.data as PaddleSubscription).id);
    return eventsOf(event);
  }
}

/** What one Paddle event means to the API; most mean nothing. */
export function eventsOf(event: PaddleEvent): ProviderEvent[] {
  const at = new Date(event.occurred_at);
  const base = { eventId: event.event_id, at };
  switch (event.event_type) {
    case "subscription.updated":
    case "subscription.resumed":
    case "subscription.past_due":
    case "subscription.canceled": {
      const sub = event.data as PaddleSubscription;
      if (sub.status === "canceled")
        return [
          {
            ...base,
            type: "subscription.ended",
            providerSubscriptionId: sub.id,
          },
        ];
      if (sub.status === "past_due" || sub.status === "paused")
        return [
          {
            ...base,
            type: "subscription.lapsed",
            providerSubscriptionId: sub.id,
          },
        ];
      const subscription = subscriptionOf(sub, at);
      if (!subscription) return [];
      return [{ ...base, type: "subscription.changed", subscription }];
    }
    case "transaction.completed":
    case "transaction.payment_failed": {
      const txn = event.data as PaddleTransaction;
      const reason = reasonOf(txn.origin);
      const total = Number(txn.details.totals.grand_total);
      const lines = linesOf(txn.items);
      if (!txn.subscription_id || !reason || !lines.length) return [];
      if (total === 0 && reason !== "first_payment") return [];
      const payment: ProviderEvent = {
        ...base,
        type:
          event.event_type === "transaction.completed"
            ? "payment.made"
            : "payment.failed",
        providerPaymentId: txn.id,
        providerSubscriptionId: txn.subscription_id,
        lines,
        reason,
        total: {
          amount: total,
          currency: txn.currency_code.toLowerCase(),
        },
        ...(txn.billing_period && {
          periodEnd: new Date(txn.billing_period.ends_at),
        }),
      };
      if (
        payment.type !== "payment.made" ||
        reason !== "first_payment" ||
        !txn.customer_id ||
        !txn.billing_period
      )
        return [payment];
      const items = ours(txn.items);
      return [
        {
          eventId: `${event.event_id}.started`,
          at,
          type: "subscription.started",
          providerCheckoutId: txn.id,
          providerCustomerId: txn.customer_id,
          providerPaymentId: txn.id,
          subscription: {
            providerSubscriptionId: txn.subscription_id,
            items: items.map(({ product, quantity }) => ({
              product,
              quantity,
            })),
            interval: intervalOf(items[0]?.price.billing_cycle ?? null),
            currentPeriodEnd: new Date(txn.billing_period.ends_at),
            cancelAtPeriodEnd: false,
          },
        },
        payment,
      ];
    }
    case "adjustment.created":
    case "adjustment.updated": {
      const adjustment = event.data as PaddleAdjustment;
      const providerPaymentId = adjustment.transaction_id;
      if (adjustment.action === "refund" && adjustment.status === "rejected")
        return [{ ...base, type: "payment.refund_refused", providerPaymentId }];
      const chargeback = adjustment.action === "chargeback";
      if (
        (adjustment.action !== "refund" && !chargeback) ||
        adjustment.status !== "approved"
      )
        return [];
      return [
        {
          ...base,
          type: "payment.refunded",
          providerPaymentId,
          returned: {
            amount: Number(adjustment.totals.total),
            currency: adjustment.currency_code.toLowerCase(),
          },
          chargeback,
        },
      ];
    }
    default:
      return [];
  }
}
