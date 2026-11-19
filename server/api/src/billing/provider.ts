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

/** How the provider holds a subscription it was asked to keep paused. */
export type HeldAs = "paused" | "active" | "lapsed" | "ended";

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
      /** The provider's id for the checkout that was paid, as `checkout` answered it. */
      providerCheckoutId: string;
      /** Whether the buyer made the checkout themselves, rather than the API with its own key. */
      openedByBuyer: boolean;
      providerCustomerId: string;
      subscription: ProviderSubscription;
      /** The subscription's first payment, when the provider names it here. */
      providerPaymentId?: string;
    }
  | { type: "subscription.changed"; subscription: ProviderSubscription }
  /** A renewal payment failed and the provider is still trying. */
  | { type: "subscription.lapsed"; providerSubscriptionId: string }
  | { type: "subscription.ended"; providerSubscriptionId: string }
  /** The subscription is paused: the provider bills nothing until it resumes. */
  | { type: "subscription.paused"; providerSubscriptionId: string }
  | {
      type: "payment.made" | "payment.failed";
      providerPaymentId: string;
      providerSubscriptionId: string;
      /** What the payment was for: each product and its quantity once it is taken. */
      lines: InvoiceLine[];
      reason: InvoiceReason;
      /** What the provider charged, in the currency it charged in. */
      total: Money;
      /** The end of the period the payment is for, when the provider names one. */
      periodEnd?: Date;
    }
  /**
   * Money went back on a payment: a refund, asked for or not, which may come long after the
   * request, or a chargeback the payer's bank made. `returned` may be less than was paid.
   */
  | {
      type: "payment.refunded";
      providerPaymentId: string;
      returned: Money;
      chargeback: boolean;
    }
  /** The provider turned down a refund asked for, and the payment stays as it was. */
  | { type: "payment.refund_refused"; providerPaymentId: string }
);

export type PauseOutcome =
  | { paidUntil: Date; pausedAt: Date; wasCancelling: boolean }
  | { askAgainAt: Date }
  | { cancelScheduled: true }
  | { refused: "paused_elsewhere" | "not_active" };

export interface CheckoutRequest {
  /** The person paying, as `customerFor` answered; their checkout cannot be moved to another. */
  providerCustomerId: string;
  product: BillingProductId;
  interval: BillingInterval;
  quantity: number;
  /** The price the catalog sets; a provider whose prices live on its side checks it matches. */
  unit: Money;
  /** Where the provider's page sends the person back to when they finish or give up. */
  returnUrl: string;
}

/** What a subscription's current period is paid for. */
export interface Paid {
  product: BillingProductId;
  interval: BillingInterval;
  quantity: number;
}

export interface SubscriptionUpdate {
  product: BillingProductId;
  interval: BillingInterval;
  quantity: number;
  unit: Money;
}

/**
 * Why a change asked for now waits for renewal: the payment method cannot be charged mid-period,
 * a change for renewal is already held, or a different price after one was taken back this period.
 */
export type NotNow = "needs_renewal" | "held" | "swap_taken_back";

/** Whether a change was taken. One asked for now is refused when its payment does not go through. */
export type ChangeOutcome =
  { taken: true } | { taken: false; why: "payment_failed" | NotNow };

/** What a change would charge, or why it would be refused. */
export type ChangePreview =
  | { now: Money; renewal: { at: Date; amount: Money } | null }
  | { refused: NotNow };

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
  /**
   * A customer at the provider for this verified email: `made` when new, else the one the provider
   * already holds under it. Null when the provider needs an email and there is none.
   */
  customerFor(
    userId: string,
    email: string | null,
  ): Promise<{ id: string; made: boolean } | null>;
  /**
   * Moves a customer at the provider to another email, which its receipts then go to. When another
   * customer holds that email, nothing moves and that customer's id comes back.
   */
  readdress(providerCustomerId: string, email: string): Promise<string | null>;
  /** The provider's hosted page to buy a product, and its id, which its payment comes back under. */
  checkout(
    request: CheckoutRequest,
  ): Promise<{ url: string; providerCheckoutId: string }>;
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
  /**
   * What the period now running is paid for, as the provider holds it: the API's record learns of
   * a renewal only from the provider's event.
   */
  paid(subscription: HeldSubscription): Promise<Paid>;
  /** What `change` with the same input would charge now and at the next renewal, changing nothing. */
  previewChange(
    subscription: HeldSubscription,
    update: SubscriptionUpdate,
    when: ChangeWhen,
  ): Promise<ChangePreview>;
  /**
   * Drops a change waiting for renewal, so the subscription renews with what is paid for now. The
   * provider is asked even when no change is known here, since its event may not have arrived.
   */
  takeBack(
    subscription: HeldSubscription,
    paid: SubscriptionUpdate,
  ): Promise<void>;
  /** Ends this product of the subscription at the end of its period, or with `now`, at once with no credit. */
  cancel(
    subscription: HeldSubscription,
    when: "period_end" | "now",
  ): Promise<void>;
  /** Takes back a cancellation at the end of the period. */
  resume(subscription: HeldSubscription): Promise<void>;
  /**
   * Pauses the subscription at once, with no end, and answers when the period it had paid for
   * would have ended, when the provider paused it, and whether it was set to cancel, which the pause
   * drops. A cancel the provider holds that `subscription` does not is answered with
   * `cancelScheduled`, to be recorded before it is dropped. A pause the provider made before `since`
   * is someone else's and is refused, as is a subscription that is not active. Inside the half hour
   * before a renewal or a scheduled cancel the provider takes no pause, so it answers when to ask
   * again instead, by which time the renewal or the cancel has gone through.
   */
  pause(subscription: HeldSubscription, since: Date): Promise<PauseOutcome>;
  /**
   * Resumes a paused subscription on `at`, when the provider charges a new period; at once when
   * `at` has passed. Asking again moves the day. Answers "paused" when the resume is set, or the
   * status the provider holds it in when it was no longer paused.
   */
  resumeOn(subscription: HeldSubscription, at: Date): Promise<HeldAs>;
  /**
   * Drops the resume `resumeOn` set, so the subscription stays paused. Answers "paused", or the
   * status the provider holds it in when it is no longer paused.
   */
  keepPaused(subscription: HeldSubscription): Promise<HeldAs>;
  /**
   * Asks for a payment to be refunded, in full or `amount` of it; payment.refunded says when it is.
   * Asking twice for one payment refunds it once.
   */
  refund(providerPaymentId: string, amount?: number): Promise<void>;
  /**
   * The provider's own page for one subscription's payment method, where it has one, and only
   * while the subscription is under the customer the API holds for it.
   */
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
