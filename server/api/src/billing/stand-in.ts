import {
  createHmac,
  randomBytes,
  randomUUID,
  timingSafeEqual,
} from "node:crypto";
import type { BillingInterval, Money, PaymentCard } from "@sikemux/protocol";

import { unitPrice } from "./catalog.ts";
import type {
  BillingProvider,
  CheckoutRequest,
  HeldSubscription,
  ProviderEvent,
  ProviderRequest,
  SubscriptionUpdate,
} from "./provider.ts";

const SIGNATURE_HEADER = "x-stand-in-signature";

function periodEnd(from: Date, interval: BillingInterval): Date {
  const end = new Date(from);
  if (interval === "month") end.setUTCMonth(end.getUTCMonth() + 1);
  else end.setUTCFullYear(end.getUTCFullYear() + 1);
  return end;
}

function periodStart(end: Date, interval: BillingInterval): Date {
  const start = new Date(end);
  if (interval === "month") start.setUTCMonth(start.getUTCMonth() - 1);
  else start.setUTCFullYear(start.getUTCFullYear() - 1);
  return start;
}

function cents(amount: number): Money {
  return { amount: Math.max(0, Math.round(amount)), currency: "usd" };
}

function revive(_key: string, value: unknown) {
  return typeof value === "string" &&
    /^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(value)
    ? new Date(value)
    : value;
}

/**
 * A payment provider for development and tests, with no account anywhere. Its checkout page
 * pays at once, and every call answers with the events a real provider would send, signed and
 * delivered to the API's own webhook before the call returns.
 */
export class StandInProvider implements BillingProvider {
  readonly name = "stand-in";
  private readonly secret = randomBytes(32);
  private readonly checkouts = new Map<string, CheckoutRequest>();
  private readonly paying = new Set<string>();
  private readonly checkoutBase: string;
  private deliver: (request: ProviderRequest) => Promise<void> = () =>
    Promise.reject(new Error("The stand-in provider has no webhook yet."));

  constructor(apiOrigin: string) {
    this.checkoutBase = `${apiOrigin}/v1/billing/stand-in/checkout/`;
  }

  /** Where its events go: the same handler the API's webhook route uses. */
  deliverTo(deliver: (request: ProviderRequest) => Promise<void>) {
    this.deliver = deliver;
  }

  private sign(body: string): string {
    return createHmac("sha256", this.secret).update(body).digest("hex");
  }

  private async send(...events: ProviderEvent[]) {
    const body = JSON.stringify(events);
    const signature = this.sign(body);
    await this.deliver({
      body,
      header: (name) =>
        name.toLowerCase() === SIGNATURE_HEADER ? signature : undefined,
    });
  }

  private event<T extends ProviderEvent["type"]>(type: T, at = new Date()) {
    return { eventId: `evt_${randomUUID()}`, at, type };
  }

  checkout(request: CheckoutRequest) {
    const token = randomBytes(16).toString("hex");
    this.checkouts.set(token, request);
    return Promise.resolve({ url: this.checkoutBase + token });
  }

  /** The checkout page: pays for what it was opened for and answers where to send the person. */
  async complete(token: string): Promise<string | null> {
    const request = this.checkouts.get(token);
    if (!request) return null;
    this.checkouts.delete(token);
    const at = new Date();
    const providerSubscriptionId = `sub_${randomUUID()}`;
    const providerCustomerId =
      request.providerCustomerId ?? `cus_${randomUUID()}`;
    this.paying.add(providerCustomerId);
    await this.send(
      {
        ...this.event("subscription.started", at),
        customerRef: request.customerRef,
        providerCustomerId,
        subscription: {
          providerSubscriptionId,
          product: request.product,
          interval: request.interval,
          quantity: request.quantity,
          currentPeriodEnd: periodEnd(at, request.interval),
          cancelAtPeriodEnd: false,
        },
      },
      {
        ...this.event("payment.made", at),
        providerPaymentId: `pay_${randomUUID()}`,
        providerSubscriptionId,
        product: request.product,
        quantity: request.quantity,
        reason: "first_payment",
        total: cents(request.unit.amount * request.quantity),
      },
    );
    return request.returnUrl;
  }

  async change(held: HeldSubscription, update: SubscriptionUpdate) {
    const now = new Date();
    const oldUnit = unitPrice(held.product, held.interval)?.amount ?? 0;
    const oldTotal = oldUnit * held.quantity;
    const newTotal = update.unit.amount * update.quantity;
    const start = periodStart(held.currentPeriodEnd, held.interval);
    const left = Math.max(
      0,
      (held.currentPeriodEnd.getTime() - now.getTime()) /
        (held.currentPeriodEnd.getTime() - start.getTime()),
    );
    const restarts = update.interval !== held.interval;
    const due = restarts
      ? newTotal - oldTotal * left
      : (newTotal - oldTotal) * left;
    const events: ProviderEvent[] = [
      {
        ...this.event("subscription.changed", now),
        subscription: {
          providerSubscriptionId: held.providerSubscriptionId,
          product: update.product,
          interval: update.interval,
          quantity: update.quantity,
          currentPeriodEnd: restarts
            ? periodEnd(now, update.interval)
            : held.currentPeriodEnd,
          cancelAtPeriodEnd: held.cancelAtPeriodEnd,
        },
      },
    ];
    if (due >= 1)
      events.push({
        ...this.event("payment.made", now),
        providerPaymentId: `pay_${randomUUID()}`,
        providerSubscriptionId: held.providerSubscriptionId,
        product: update.product,
        quantity: update.quantity,
        reason: "change",
        total: cents(due),
      });
    await this.send(...events);
  }

  private async setCancel(held: HeldSubscription, cancelAtPeriodEnd: boolean) {
    await this.send({
      ...this.event("subscription.changed"),
      subscription: {
        providerSubscriptionId: held.providerSubscriptionId,
        product: held.product,
        interval: held.interval,
        quantity: held.quantity,
        currentPeriodEnd: held.currentPeriodEnd,
        cancelAtPeriodEnd,
      },
    });
  }

  async cancel(held: HeldSubscription, when: "period_end" | "now") {
    if (when === "period_end") return this.setCancel(held, true);
    await this.send({
      ...this.event("subscription.ended"),
      providerSubscriptionId: held.providerSubscriptionId,
    });
  }

  resume(held: HeldSubscription) {
    return this.setCancel(held, false);
  }

  async refund(providerPaymentId: string) {
    await this.send({
      ...this.event("payment.refunded"),
      providerPaymentId,
    });
  }

  portal() {
    return Promise.resolve(null);
  }

  receipt() {
    return Promise.resolve(null);
  }

  /** The test card every stand-in checkout pays with. */
  card(providerCustomerId: string): Promise<PaymentCard | null> {
    return Promise.resolve(
      this.paying.has(providerCustomerId)
        ? {
            brand: "visa",
            last4: "4242",
            expMonth: 12,
            expYear: new Date().getUTCFullYear() + 3,
          }
        : null,
    );
  }

  readEvents(request: ProviderRequest): Promise<ProviderEvent[] | null> {
    const given = Buffer.from(request.header(SIGNATURE_HEADER) ?? "", "hex");
    const expected = Buffer.from(this.sign(request.body), "hex");
    if (given.length !== expected.length || !timingSafeEqual(given, expected))
      return Promise.resolve(null);
    return Promise.resolve(JSON.parse(request.body, revive) as ProviderEvent[]);
  }
}
