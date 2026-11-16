import type {
  BillingInterval,
  BillingProductId,
  ChangeWhen,
  InvoiceLine,
  InvoiceReason,
  Money,
  PaymentCard,
} from "@sikemux/protocol";

/** A subscription as the API holds it, for a provider to act on. */
export interface HeldSubscription {
  providerSubscriptionId: string;
  providerCustomerId: string | null;
  product: BillingProductId;
  interval: BillingInterval;
  quantity: number;
  currentPeriodEnd: Date;
  cancelAtPeriodEnd: boolean;
}

/** A subscription row as a provider acts on it. */
export function heldSubscription(
  row: {
    provider_subscription_id: string;
    product: BillingProductId;
    interval: BillingInterval;
    quantity: number;
    current_period_end: Date;
    cancel_at_period_end: boolean;
  },
  providerCustomerId: string | null,
): HeldSubscription {
  return {
    providerSubscriptionId: row.provider_subscription_id,
    providerCustomerId,
    product: row.product,
    interval: row.interval,
    quantity: row.quantity,
    currentPeriodEnd: row.current_period_end,
    cancelAtPeriodEnd: row.cancel_at_period_end,
  };
}

/** One product under a provider's subscription, with a change waiting for renewal if one is. */
export interface ProviderItem {
  product: BillingProductId;
  quantity: number;
  atRenewal?: {
    product: BillingProductId;
    quantity: number;
    interval: BillingInterval;
  };
}

/**
 * What a subscription is now, as the provider reports it: every product it holds, one for a
 * provider that gives each product its own subscription. A product left out has ended.
 */
export interface ProviderSubscription {
  providerSubscriptionId: string;
  items: ProviderItem[];
  interval: BillingInterval;
  currentPeriodEnd: Date;
  cancelAtPeriodEnd: boolean;
}

/**
 * The few things the API acts on, whatever the provider calls them. Each carries the provider's
 * own event id, which the API remembers so an event sent again changes nothing, and when the
 * provider says it happened, so an older one arriving late cannot undo a newer.
 */
export type ProviderEvent = {
  eventId: string;
  at: Date;
} & (
  | {
      type: "subscription.started";
      /** The billing customer id the checkout passed, which the provider hands back. */
      customerRef: string;
      providerCustomerId: string;
      subscription: ProviderSubscription;
    }
  | { type: "subscription.changed"; subscription: ProviderSubscription }
  /** A renewal payment failed and the provider is still trying. */
  | { type: "subscription.lapsed"; providerSubscriptionId: string }
  | { type: "subscription.ended"; providerSubscriptionId: string }
  | {
      type: "payment.made" | "payment.failed";
      providerPaymentId: string;
      providerSubscriptionId: string;
      /** What the payment was for: each product and its quantity once it is taken. */
      lines: InvoiceLine[];
      reason: InvoiceReason;
      /** What the provider charged, in the currency it charged in. */
      total: Money;
    }
  /** The refund asked for went through; it may come long after the request. */
  | { type: "payment.refunded"; providerPaymentId: string }
);

export interface CheckoutRequest {
  /** The billing customer, handed back on subscription.started. */
  customerRef: string;
  /** Set once the provider has made a customer for this payer. */
  providerCustomerId: string | null;
  product: BillingProductId;
  interval: BillingInterval;
  quantity: number;
  /** The price the catalog sets; a provider whose prices live on its side checks it matches. */
  unit: Money;
  /** Where the provider's page sends the person back to when they finish or give up. */
  returnUrl: string;
}

export interface SubscriptionUpdate {
  product: BillingProductId;
  interval: BillingInterval;
  quantity: number;
  unit: Money;
}

/** Whether a change was taken. One asked for now is refused when its payment does not go through. */
export type ChangeOutcome =
  { taken: true } | { taken: false; why: "payment_failed" | "needs_renewal" };

/** A request the provider sent to the API's webhook, as it arrived. */
export interface ProviderRequest {
  body: string;
  header(name: string): string | undefined;
}

/**
 * One payment provider. Nothing here changes the API's records: each call asks the provider, and
 * the provider's events, read by `readEvents`, are what change them. A call may return before
 * those events arrive.
 */
export interface BillingProvider {
  /** The name kept beside each customer, such as "stand-in". */
  readonly name: string;
  /**
   * How long the provider keeps sending an event the API answered "not yet". An event about
   * something unknown is answered so until it is older than this, then taken as about something
   * erased.
   */
  readonly eventRetrySpanMs: number;
  /** The provider's hosted page to buy a product. */
  checkout(request: CheckoutRequest): Promise<{ url: string }>;
  /**
   * A new quantity, product or interval for one product of a subscription. `now` charges the
   * prorated difference and changes nothing unless that payment goes through; `renewal` takes
   * over when the period ends.
   */
  change(
    subscription: HeldSubscription,
    update: SubscriptionUpdate,
    when: ChangeWhen,
  ): Promise<ChangeOutcome>;
  /** Ends this product of the subscription at the end of its period, or with `now`, at once with no credit. */
  cancel(
    subscription: HeldSubscription,
    when: "period_end" | "now",
  ): Promise<void>;
  /** Takes back a cancellation at the end of the period. */
  resume(subscription: HeldSubscription): Promise<void>;
  /** Asks for a payment to be refunded in full; payment.refunded says when it is. */
  refund(providerPaymentId: string): Promise<void>;
  /** The provider's own page for one subscription's payment method, where it has one. */
  portal(subscription: HeldSubscription): Promise<{ url: string } | null>;
  /** The provider's receipt for one payment, where it has one. */
  receipt(providerPaymentId: string): Promise<{ url: string } | null>;
  /**
   * The card one subscription pays with, asked each time so no card detail is kept here. Scoped to
   * the subscription because a provider's customer may also be paying for something else.
   */
  card(subscription: HeldSubscription): Promise<PaymentCard | null>;
  /** The events in a webhook request, or null when its signature is not the provider's. */
  readEvents(request: ProviderRequest): Promise<ProviderEvent[] | null>;
}
