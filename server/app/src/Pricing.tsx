import type {
  BillingCatalog,
  BillingInterval,
  BillingProduct,
  PlanFeature,
} from "@sikemux/protocol";
import { IconLock } from "@shared/ui/Icons";
import { useEffect, useState } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./Billing.tsx";
import { BilledSwitch } from "./BilledSwitch.tsx";
import {
  FEATURE_WORDS,
  featuresAdded,
  money,
  PER,
  priceOf,
  SHED_WORDS,
  UNIT_WORD,
} from "./money.ts";
import { PlanCard, type CardLine } from "./PlanCard.tsx";
import { PageHead, Section } from "./ui.tsx";

type Built = "" | "partly" | "not built" | "later";

/** How far each feature is built. These marks are for us, and change as the work lands. */
const BUILT: Record<PlanFeature, Built> = {
  sync: "partly",
  move_work: "partly",
  history_search: "not built",
  own_triggers: "partly",
  org_hosts: "partly",
  org_shared_setup: "partly",
  org_model_keys: "partly",
  org_github: "not built",
  org_bot_triggers: "partly",
  chat_handover: "partly",
  org_roles: "not built",
  org_policy: "partly",
};

/** Free's features are everyone's, so the catalog does not list them. */
const FREE: [string, Built][] = [
  ["The whole app on Mac and Linux", ""],
  ["Every harness and every model: frontier, coding plans, local", ""],
  ["Unlimited agents and mixed-vendor agent teams", ""],
  [
    "Limits for every plan you pay for, and a chat that carries on in another agent when one runs out",
    "partly",
  ],
  [
    "@Chief per project: calls only when you must decide, Overrule and Undo, why with sources, rules said twice become checks",
    "",
  ],
  ["Auto, line attribution", ""],
  ["Git, browser, plugins, notch, dictation", ""],
  [
    "Every computer you own, linked on one account and reachable from desktop and phone: your laptop, a headless box, one click into your own Hetzner or DigitalOcean",
    "partly",
  ],
  ["Your vault, on every host", ""],
];

const TEAM_LATER = ["Review check on PRs", "Pin-to-agent widget"];

const TAGS: Record<string, string> = {
  free: "The distribution",
  team: "What a company pays for",
};

type Load =
  | { state: "loading" }
  | { state: "failed"; problem: string }
  | { state: "loaded"; catalog: BillingCatalog };

export function PricingPage() {
  const client = useClient();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [interval, setInterval] = useState<BillingInterval>("month");

  useEffect(() => {
    let current = true;
    void client("see the prices")
      .then((routes) => routes.getBillingCatalog())
      .then(
        (catalog) => current && setLoad({ state: "loaded", catalog }),
        (error: unknown) =>
          current &&
          setLoad({
            state: "failed",
            problem: `${problemText(error)} Reload the page to try again.`,
          }),
      );
    return () => {
      current = false;
    };
  }, [client]);

  return (
    <>
      <div className="staff-only">
        <IconLock size={13} />
        Staff only. Not linked from sikemux.com. Build marks are for us.
      </div>
      <PageHead title="Pricing">
        The app stays free and MIT. Paying is only for what costs us money to
        run.
      </PageHead>
      {load.state === "loading" ? (
        <p className="quiet">Loading prices…</p>
      ) : load.state === "failed" ? (
        <p className="problem" role="alert">
          {load.problem}
        </p>
      ) : (
        <div className="groups">
          <Plans
            catalog={load.catalog}
            interval={interval}
            setInterval={setInterval}
          />
          <Shed catalog={load.catalog} interval={interval} />
        </div>
      )}
    </>
  );
}

function Plans({
  catalog,
  interval,
  setInterval,
}: {
  catalog: BillingCatalog;
  interval: BillingInterval;
  setInterval: (interval: BillingInterval) => void;
}) {
  return (
    <Section
      label="Plans"
      aside={<BilledSwitch interval={interval} onChange={setInterval} />}
    >
      <div className="plan-cards three">
        {catalog.plans.map((plan) => {
          const product = catalog.products.find(
            (each) => each.kind === "plan" && each.plan === plan.id,
          );
          const { before, added } = featuresAdded(catalog, plan.id);
          const lines: CardLine[] =
            plan.id === "free"
              ? FREE.map(([text, built]) => ({
                  key: text,
                  text: <Feature text={text} built={built} />,
                }))
              : [
                  ...added.map((feature) => ({
                    key: feature,
                    text: (
                      <Feature
                        text={FEATURE_WORDS[feature]}
                        built={BUILT[feature]}
                      />
                    ),
                  })),
                  ...(plan.id === "team"
                    ? TEAM_LATER.map((text) => ({
                        key: text,
                        text: <Feature text={`Later: ${text}`} built="later" />,
                      }))
                    : []),
                ];
          const marks =
            plan.id === "free"
              ? FREE.map(([, built]) => built)
              : added.map((feature) => BUILT[feature]);
          const { price, per, other } = product
            ? priced(product, interval)
            : {
                price: { amount: 0, currency: "usd" },
                per: undefined,
                other: undefined,
              };
          return (
            <PlanCard
              key={plan.id}
              name={plan.name}
              aside={`${marks.filter((built) => built === "").length} of ${marks.length} built`}
              chosen={plan.id === "pro"}
              price={price}
              per={per}
              tag={joined(other, TAGS[plan.id])}
              plus={before ? `Everything in ${before}, plus` : undefined}
              lines={lines}
            />
          );
        })}
      </div>
    </Section>
  );
}

function Shed({
  catalog,
  interval,
}: {
  catalog: BillingCatalog;
  interval: BillingInterval;
}) {
  const boxes = catalog.products.filter((product) => product.kind === "shed");
  if (boxes.length === 0) return null;
  return (
    <Section label="Shed · add-on · any plan, Free included">
      <p className="section-lede">
        A Linux box we run, joined to your account. Buy one or several by the
        box. You sign in to Claude Code and Codex on it with your own plans.
      </p>
      <div className="plan-cards three">
        {boxes.map((product) => {
          const words = SHED_WORDS[product.id];
          const { price, per, other } = priced(product, interval);
          return (
            <PlanCard
              key={product.id}
              name={
                <>
                  {product.name}
                  <Mark built="not built" />
                </>
              }
              price={price}
              per={per}
              tag={joined(other, words?.tag)}
              lines={(words?.specs ?? []).map((text) => ({ key: text, text }))}
            >
              {words ? <div className="runs-on">{words.runsOn}</div> : null}
            </PlanCard>
          );
        })}
      </div>
    </Section>
  );
}

/** The price at the chosen interval, else the one it is sold by, and the other interval's price. */
function priced(product: BillingProduct, interval: BillingInterval) {
  const shown = priceOf(product, interval) ? interval : "month";
  const other: BillingInterval = shown === "month" ? "year" : "month";
  const otherPrice = priceOf(product, other);
  return {
    price: priceOf(product, shown),
    per: [UNIT_WORD[product.unit], PER[shown]].filter(Boolean).join(" "),
    other: otherPrice
      ? `or ${money(otherPrice, { cents: false })} ${PER[other]}`
      : undefined,
  };
}

function joined(...parts: (string | undefined)[]) {
  return parts.filter(Boolean).join(" · ") || undefined;
}

function Feature({ text, built }: { text: string; built: Built }) {
  return (
    <>
      {text}
      <Mark built={built} />
    </>
  );
}

function Mark({ built }: { built: Built }) {
  return built ? (
    <span className="built" data-built={built}>
      {built}
    </span>
  ) : null;
}
