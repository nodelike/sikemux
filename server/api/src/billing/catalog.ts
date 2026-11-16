import type {
  BillingCatalog,
  BillingInterval,
  BillingPlan,
  BillingProduct,
  BillingProductId,
  Money,
  PlanFeature,
  PlanId,
} from "@sikemux/protocol";

/** How many days after a subscription's first payment it is refunded in full on request. */
export const REFUND_DAYS = 7;

const usd = (cents: number): Money => ({ amount: cents, currency: "usd" });

/** A year of a Shed box costs ten months. */
const shedPrices = (monthCents: number) => [
  { interval: "month" as const, unit: usd(monthCents) },
  { interval: "year" as const, unit: usd(monthCents * 10) },
];

export const PRODUCTS: readonly BillingProduct[] = [
  {
    id: "pro",
    name: "Pro",
    kind: "plan",
    plan: "pro",
    unit: "account",
    payer: "person",
    maxQuantity: 1,
    prices: [
      { interval: "month", unit: usd(800) },
      { interval: "year", unit: usd(8000) },
    ],
  },
  {
    id: "team",
    name: "Team",
    kind: "plan",
    plan: "team",
    unit: "seat",
    payer: "org",
    maxQuantity: 1000,
    prices: [{ interval: "month", unit: usd(2000) }],
  },
  {
    id: "shed",
    name: "Shed",
    kind: "shed",
    unit: "box",
    payer: "either",
    maxQuantity: 20,
    prices: shedPrices(1900),
  },
  {
    id: "shed_always_on",
    name: "Shed Always-on",
    kind: "shed",
    unit: "box",
    payer: "either",
    maxQuantity: 20,
    prices: shedPrices(2900),
  },
  {
    id: "shed_large",
    name: "Shed Always-on, large",
    kind: "shed",
    unit: "box",
    payer: "either",
    maxQuantity: 20,
    prices: shedPrices(4900),
  },
];

const PRO_FEATURES: readonly PlanFeature[] = [
  "sync",
  "move_work",
  "history_search",
  "own_triggers",
];

/** Team includes everything in Pro, for a seat holder on their own hosts too. */
const TEAM_FEATURES: readonly PlanFeature[] = [
  ...PRO_FEATURES,
  "org_hosts",
  "org_shared_setup",
  "org_model_keys",
  "org_github",
  "org_bot_triggers",
  "chat_handover",
  "org_roles",
  "org_policy",
];

export const PLANS: readonly BillingPlan[] = [
  { id: "free", name: "Free", features: [] },
  { id: "pro", name: "Pro", features: [...PRO_FEATURES] },
  { id: "team", name: "Team", features: [...TEAM_FEATURES] },
];

export const ALL_FEATURES: readonly PlanFeature[] = TEAM_FEATURES;

export const CATALOG: BillingCatalog = {
  products: [...PRODUCTS],
  plans: [...PLANS],
  refundDays: REFUND_DAYS,
};

export function product(id: BillingProductId): BillingProduct {
  const found = PRODUCTS.find((each) => each.id === id);
  if (!found) throw new Error(`No product ${id} in the catalog.`);
  return found;
}

export function featuresOf(plan: PlanId): readonly PlanFeature[] {
  return PLANS.find((each) => each.id === plan)?.features ?? [];
}

/** What one unit of a product costs each interval, or null when it is not sold by that interval. */
export function unitPrice(
  id: BillingProductId,
  interval: BillingInterval,
): Money | null {
  return (
    product(id).prices.find((price) => price.interval === interval)?.unit ??
    null
  );
}
