import {
  createHmac,
  randomBytes,
  randomUUID,
  timingSafeEqual,
} from "node:crypto";
import type {
  BillingInterval,
  ChangeWhen,
  Money,
  PaymentCard,
} from "@sikemux/protocol";

import { unitPrice } from "./catalog.ts";
import type {
  BillingProvider,
  ChangeOutcome,
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
 * delivered to the API's own webhook before the call returns. Like some real providers, it gives
 * each product its own subscription.
 */
export class StandInProvider implements BillingProvider {
  readonly name = "stand-in";
  readonly eventRetrySpanMs = 3 * 24 * 60 * 60_000;
  private readonly secret = randomBytes(32);
  private readonly checkouts = new Map<string, CheckoutRequest>();
  private readonly paying = new Set<string>();
  private readonly heldRefunds: string[] = [];
  private readonly paid = new Map<string, Money>();
  /** Refuses every charge taken mid-period, as a payment method that cannot be charged then. */
  declineChanges = false;
  /** Fails every cancellation, as a provider that cannot be reached. */
  refuseCancels = false;
  /** Keeps refunds waiting for approval, as a provider that approves each by hand, until `approveRefunds`. */
  holdRefunds = false;
  /** Turns down every refund, as a provider whose review refuses it. */
  refuseRefunds = false;
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
    for (const event of events)
      if (event.type === "payment.made")
        this.paid.set(event.providerPaymentId, event.total);
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
          items: [{ product: request.product, quantity: request.quantity }],
          interval: request.interval,
          currentPeriodEnd: periodEnd(at, request.interval),
          cancelAtPeriodEnd: false,
        },
      },
      {
        ...this.event("payment.made", at),
        providerPaymentId: `pay_${randomUUID()}`,
        providerSubscriptionId,
        lines: [{ product: request.product, quantity: request.quantity }],
        reason: "first_payment",
        total: cents(request.unit.amount * request.quantity),
      },
    );
    return request.returnUrl;
  }

  async change(
    held: HeldSubscription,
    update: SubscriptionUpdate,
    when: ChangeWhen,
  ): Promise<ChangeOutcome> {
    const now = new Date();
    if (when === "renewal") {
      await this.send({
        ...this.event("subscription.changed", now),
        subscription: {
          ...this.subscriptionOf(held),
          items: [
            {
              product: held.product,
              quantity: held.quantity,
              atRenewal: {
                product: update.product,
                quantity: update.quantity,
                interval: update.interval,
              },
            },
          ],
        },
      });
      return { taken: true };
    }
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
    const payment = {
      providerPaymentId: `pay_${randomUUID()}`,
      providerSubscriptionId: held.providerSubscriptionId,
      lines: [{ product: update.product, quantity: update.quantity }],
      reason: "change" as const,
      total: cents(due),
    };
    if (due >= 1 && this.declineChanges) {
      await this.send({ ...this.event("payment.failed", now), ...payment });
      return { taken: false, why: "payment_failed" };
    }
    const events: ProviderEvent[] = [
      {
        ...this.event("subscription.changed", now),
        subscription: {
          providerSubscriptionId: held.providerSubscriptionId,
          items: [{ product: update.product, quantity: update.quantity }],
          interval: update.interval,
          currentPeriodEnd: restarts
            ? periodEnd(now, update.interval)
            : held.currentPeriodEnd,
          cancelAtPeriodEnd: held.cancelAtPeriodEnd,
        },
      },
    ];
    if (due >= 1)
      events.push({ ...this.event("payment.made", now), ...payment });
    await this.send(...events);
    return { taken: true };
  }

  private subscriptionOf(held: HeldSubscription) {
    return {
      providerSubscriptionId: held.providerSubscriptionId,
      items: [{ product: held.product, quantity: held.quantity }],
      interval: held.interval,
      currentPeriodEnd: held.currentPeriodEnd,
      cancelAtPeriodEnd: held.cancelAtPeriodEnd,
    };
  }

  private async setCancel(held: HeldSubscription, cancelAtPeriodEnd: boolean) {
    await this.send({
      ...this.event("subscription.changed"),
      subscription: { ...this.subscriptionOf(held), cancelAtPeriodEnd },
    });
  }

  async cancel(held: HeldSubscription, when: "period_end" | "now") {
    if (this.refuseCancels)
      throw new Error("The stand-in provider cannot be reached.");
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
    if (this.holdRefunds) this.heldRefunds.push(providerPaymentId);
    else await this.settleRefund(providerPaymentId);
  }

  async approveRefunds() {
    for (const providerPaymentId of this.heldRefunds.splice(0))
      await this.settleRefund(providerPaymentId);
  }

  private settleRefund(providerPaymentId: string) {
    if (this.refuseRefunds)
      return this.send({
        ...this.event("payment.refund_refused"),
        providerPaymentId,
      });
    return this.send({
      ...this.event("payment.refunded"),
      providerPaymentId,
      returned: this.paid.get(providerPaymentId) ?? cents(0),
      chargeback: false,
    });
  }

  portal() {
    return Promise.resolve(null);
  }

  receipt() {
    return Promise.resolve(null);
  }

  /** The test card every stand-in checkout pays with. */
  card({ providerCustomerId }: HeldSubscription): Promise<PaymentCard | null> {
    return Promise.resolve(
      providerCustomerId && this.paying.has(providerCustomerId)
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
