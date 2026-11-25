import type {
  BillingCatalog,
  BillingInterval,
  BillingProduct,
  BillingProductId,
  Money,
  PlanFeature,
  PlanId,
} from "@sikemux/protocol";

export const FEATURE_WORDS: Record<PlanFeature, string> = {
  sync: "Your hosts work as one: settings, skills and memory stay in sync",
  move_work: "Move a chat between your own hosts",
  history_search: "History search across hosts",
  own_triggers: "Schedules and webhooks you set up yourself",
  org_hosts:
    "Org hosts: the company's Linux hosts, where each person gets their own place in one click",
  org_shared_setup:
    "Shared skills, instructions, subagents and MCP servers for the whole org",
  org_model_keys:
    "Org model keys; your work sign-ins, personal rules and browser stay yours",
  org_github:
    "Connect GitHub once; git works on every host and agents never see a token",
  org_bot_triggers:
    "Triggers run as the org bot and ask the people or groups they name",
  chat_handover: "Hand a task to a teammate, with its decisions and context",
  org_roles: "Owner, admin, member; groups; email domains",
  org_policy: "Policy, audit log, one bill",
};

/** Team features the preview does not open to everyone in it. */
const NOT_IN_PREVIEW = new Set<PlanFeature>([
  "org_hosts",
  "org_shared_setup",
  "org_model_keys",
  "org_github",
  "org_bot_triggers",
]);

export const inPreview = (feature: PlanFeature) => !NOT_IN_PREVIEW.has(feature);

/** What a plan adds over the one before it in the catalog. */
export function featuresAdded(catalog: BillingCatalog, plan: PlanId) {
  const index = catalog.plans.findIndex((each) => each.id === plan);
  const before = new Set(catalog.plans[index - 1]?.features ?? []);
  return {
    before: catalog.plans[index - 1]?.name,
    added: (catalog.plans[index]?.features ?? []).filter(
      (feature) => !before.has(feature),
    ),
  };
}

export function money(value: Money, { cents = true } = {}): string {
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: value.currency.toUpperCase(),
    minimumFractionDigits: cents || value.amount % 100 ? 2 : 0,
  }).format(value.amount / 100);
}

export function times(value: Money, count: number): Money {
  return { amount: value.amount * count, currency: value.currency };
}

export function productOf(
  catalog: BillingCatalog,
  id: BillingProductId,
): BillingProduct | undefined {
  return catalog.products.find((each) => each.id === id);
}

export function priceOf(
  product: BillingProduct | undefined,
  interval: BillingInterval,
): Money | undefined {
  return product?.prices.find((each) => each.interval === interval)?.unit;
}

export const PER: Record<BillingInterval, string> = {
  month: "a month",
  year: "a year",
};

export const UNIT_WORD: Record<BillingProduct["unit"], string> = {
  account: "",
  seat: "a seat",
  box: "a box",
};

/** A price the way a person says it, like "$20 a seat a month". */
export function priceText(
  product: BillingProduct,
  interval: BillingInterval,
): string | undefined {
  const unit = priceOf(product, interval);
  if (!unit) return undefined;
  return [money(unit, { cents: false }), UNIT_WORD[product.unit], PER[interval]]
    .filter(Boolean)
    .join(" ");
}

export type ShedWords = {
  tag: string;
  line: string;
  specs: string[];
  runsOn: string;
};

export const SHED_WORDS: Partial<Record<BillingProductId, ShedWords>> = {
  shed: {
    tag: "Sleeps when idle",
    line: "Up to 8 GiB of memory · wakes in about a second",
    specs: [
      "Agents keep working with your laptop shut",
      "Sleeps when no agent is working and nobody is connected, wakes in about a second",
      "Up to 8 GiB of memory",
      "No hours counted",
    ],
    runsOn: "Runs on boxd, which bills us on use",
  },
  shed_always_on: {
    tag: "Never sleeps",
    line: "4 vCPU, 8 GB of memory, 75 GB of disk",
    specs: [
      "4 vCPU, 8 GB of memory, 75 GB of disk",
      "For agents that work round the clock",
      "Dev servers and schedules stay up",
    ],
    runsOn: "Runs on a fixed-price VM",
  },
  shed_large: {
    tag: "Never sleeps",
    line: "8 vCPU, 24 GB of memory, 200 GB of disk",
    specs: [
      "8 vCPU, 24 GB of memory, 200 GB of disk",
      "Room for big builds and many agents",
    ],
    runsOn: "Runs on a fixed-price VM",
  },
};
