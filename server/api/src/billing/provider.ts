import type {
  BillingInterval,
  BillingProductId,
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

/** What a subscription is now, as the provider reports it. */
export interface ProviderSubscription {
  providerSubscriptionId: string;
  product: BillingProductId;
  interval: BillingInterval;
  quantity: number;
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
      /** What the payment was for: the subscription's product and quantity once it is taken. */
      product: BillingProductId;
      quantity: number;
      reason: InvoiceReason;
      /** What the provider charged, in the currency it charged in. */
      total: Money;
    }
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
  /** The provider's hosted page to buy a product. */
  checkout(request: CheckoutRequest): Promise<{ url: string }>;
  /** A new quantity, product or interval, prorated for the rest of the period. */
  change(
    subscription: HeldSubscription,
    update: SubscriptionUpdate,
  ): Promise<void>;
  /** Ends the subscription at the end of its period, or with `now`, at once with no credit. */
  cancel(
    subscription: HeldSubscription,
    when: "period_end" | "now",
  ): Promise<void>;
  /** Takes back a cancellation at the end of the period. */
  resume(subscription: HeldSubscription): Promise<void>;
  /** Refunds a payment in full. */
  refund(providerPaymentId: string): Promise<void>;
  /** The provider's own page for a customer's payment methods and receipts, where it has one. */
  portal(providerCustomerId: string): Promise<{ url: string } | null>;
  /** The provider's receipt for one payment, where it has one. */
  receipt(providerPaymentId: string): Promise<{ url: string } | null>;
  /** The card a customer pays with, asked each time so no card detail is kept here. */
  card(providerCustomerId: string): Promise<PaymentCard | null>;
  /** The events in a webhook request, or null when its signature is not the provider's. */
  readEvents(request: ProviderRequest): Promise<ProviderEvent[] | null>;
}
