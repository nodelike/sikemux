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
  customer_ref?: string;
  held?: Held;
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
  custom_data: CustomData | null;
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

function productOf(price: PaddlePrice): BillingProductId {
  const tag = priceTag(price);
  if (!tag)
    throw new Error(`Paddle's price ${price.id} is not one the setup made.`);
  return tag.product;
}

function linesOf(
  items: readonly { quantity: number; price: PaddlePrice }[],
): InvoiceLine[] {
  return (
    items
      // A change lists what it replaced with a negative quantity.
      .filter((item) => item.quantity > 0)
      .map((item) => ({
        product: productOf(item.price),
        quantity: item.quantity,
      }))
  );
}

/**
 * The customer a checkout of ours was opened for. Anyone holding the client token can open a
 * checkout with Paddle.js and any custom data, so only a transaction our API made is believed.
 */
function ourCustomerRef(txn: PaddleTransaction): string | null {
  return txn.origin === "api" ? (txn.custom_data?.customer_ref ?? null) : null;
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

function subscriptionOf(
  sub: PaddleSubscription,
  at: Date,
): ProviderSubscription {
  const live = sub.items.filter((item) => item.status !== "inactive");
  const items: ProviderItem[] = live.map((item) => ({
    product: productOf(item.price),
    quantity: item.quantity,
  }));
  const interval = intervalOf(sub.billing_cycle);
  const ends =
    sub.current_billing_period?.ends_at ??
    sub.next_billed_at ??
    sub.scheduled_change?.effective_at;
  if (!ends) throw new Error(`Paddle's subscription ${sub.id} has no period.`);
  const reported: ProviderSubscription = {
    providerSubscriptionId: sub.id,
    items,
    interval,
    currentPeriodEnd: new Date(ends),
    cancelAtPeriodEnd: sub.scheduled_change?.action === "cancel",
  };
  const held = sub.custom_data?.held;
  const [now] = items;
  if (!held || !now || items.length !== 1 || new Date(held.until) <= at)
    return reported;
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
  private secret: { value: string; at: number } | null;
  private prices: { all: PaddlePrice[]; at: number } | null = null;
  private clientToken: string | null = null;

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

  private notSetUp(what: string): never {
    this.log.error(`Paddle has no ${what}: run the paddle-setup command`);
    throw new ApiFailure(
      503,
      "unavailable",
      "The payment provider is not set up yet.",
    );
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
    if (
      this.secret &&
      (!again || this.now() - this.secret.at < REFETCH_AFTER_MS)
    )
      return this.secret.value;
    const settings = await this.api.list<{
      destination: string;
      active: boolean;
      endpoint_secret_key: string;
    }>("/notification-settings");
    const found = settings.find(
      (setting) => setting.destination === this.webhookUrl && setting.active,
    );
    this.secret = found
      ? { value: found.endpoint_secret_key, at: this.now() }
      : null;
    return this.secret?.value ?? null;
  }

  /** What the checkout page opens Paddle's checkout with. The token is public by Paddle's design. */
  async checkoutPage(): Promise<{ clientToken: string; sandbox: boolean }> {
    if (!this.clientToken) {
      const tokens = await this.api.list<{
        name: string;
        status: string;
        token: string;
      }>("/client-tokens?status=active");
      this.clientToken =
        tokens.find((token) => token.name === CLIENT_TOKEN_NAME)?.token ??
        this.notSetUp("client token");
    }
    return { clientToken: this.clientToken, sandbox: this.sandbox };
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
        custom_data: { customer_ref: request.customerRef },
        ...(request.providerCustomerId
          ? { customer_id: request.providerCustomerId }
          : {}),
      },
    );
    if (!transaction.checkout?.url)
      throw new Error("Paddle made a checkout with no page.");
    return { url: transaction.checkout.url };
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

  async change(
    held: HeldSubscription,
    update: SubscriptionUpdate,
    when: ChangeWhen,
  ): Promise<ChangeOutcome> {
    const price = await this.priceFor(
      update.product,
      update.interval,
      update.unit,
    );
    const sub = await this.subscription(held.providerSubscriptionId);
    const items = [{ price_id: price.id, quantity: update.quantity }];
    const customerRef = sub.custom_data?.customer_ref;
    const custom = customerRef ? { customer_ref: customerRef } : {};
    if (when === "renewal") {
      const until = held.currentPeriodEnd.toISOString();
      await this.api.request("PATCH", `/subscriptions/${sub.id}`, {
        items,
        proration_billing_mode: "do_not_bill",
        custom_data: {
          ...custom,
          held: {
            product: held.product,
            quantity: held.quantity,
            interval: held.interval,
            until,
          } satisfies Held,
        },
        ...(update.interval === held.interval ? {} : { next_billed_at: until }),
      });
      return { taken: true };
    }
    if (await this.paysByUpi(sub.id))
      return { taken: false, why: "needs_renewal" };
    try {
      await this.api.request("PATCH", `/subscriptions/${sub.id}`, {
        items,
        proration_billing_mode: "prorated_immediately",
        on_payment_failure: "prevent_change",
        custom_data: custom,
      });
    } catch (err) {
      if (
        err instanceof PaddleError &&
        err.code === "subscription_payment_declined"
      )
        return { taken: false, why: "payment_failed" };
      throw err;
    }
    return { taken: true };
  }

  async cancel(held: HeldSubscription, when: "period_end" | "now") {
    try {
      await this.api.request(
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
    await this.api.request(
      "PATCH",
      `/subscriptions/${held.providerSubscriptionId}`,
      {
        scheduled_change: null,
      },
    );
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
    const secret =
      (await this.webhookSecret()) ?? this.notSetUp("webhook destination");
    if (!signedBy(header, request.body, secret, this.now())) {
      const fresh = await this.webhookSecret(true);
      if (
        !fresh ||
        fresh === secret ||
        !signedBy(header, request.body, fresh, this.now())
      )
        return null;
    }
    return eventsOf(JSON.parse(request.body) as PaddleEvent);
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
      return [
        {
          ...base,
          type: "subscription.changed",
          subscription: subscriptionOf(sub, at),
        },
      ];
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
      };
      if (
        payment.type !== "payment.made" ||
        reason !== "first_payment" ||
        !txn.customer_id ||
        !txn.billing_period
      )
        return [payment];
      const [first] = txn.items;
      return [
        {
          eventId: `${event.event_id}.started`,
          at,
          type: "subscription.started",
          customerRef: ourCustomerRef(txn),
          providerCustomerId: txn.customer_id,
          providerPaymentId: txn.id,
          subscription: {
            providerSubscriptionId: txn.subscription_id,
            items: txn.items.map((item) => ({
              product: productOf(item.price),
              quantity: item.quantity,
            })),
            interval: intervalOf(first?.price.billing_cycle ?? null),
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
