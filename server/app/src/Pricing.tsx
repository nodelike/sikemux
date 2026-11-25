import { IconCheck, IconClock, IconMinus, IconRefresh } from "@shared/ui/Icons";
import type {
  BillingCatalog,
  BillingInterval,
  BillingProduct,
  Org,
  PlanFeature,
} from "@sikemux/protocol";
import { useState } from "react";

import { problemText } from "./api.ts";
import { OrgAvatar } from "./Avatar.tsx";
import { BilledSwitch } from "./BilledSwitch.tsx";
import { Stepper, useCheckout, useClient } from "./Billing.tsx";
import { iconMark } from "./marks.ts";
import {
  FEATURE_WORDS,
  featuresAdded,
  money,
  PER,
  priceOf,
  productOf,
  SHED_WORDS,
  times,
  UNIT_WORD,
} from "./money.ts";
import { useLoad } from "./orgs.ts";
import { PlanCard, type BuiltState, type CardLine } from "./PlanCard.tsx";
import {
  Btn,
  Dialog,
  Field,
  Nothing,
  Page,
  Problem,
  Row,
  Rows,
  Section,
  useAction,
  useToast,
} from "./ui.tsx";
import "./billing.css";

/** How far each feature is built. These marks are for us, and change as the work lands. */
const BUILT: Record<PlanFeature, BuiltState> = {
  sync: "partly",
  move_work: "built",
  history_search: "built",
  own_triggers: "built",
  org_hosts: "no",
  org_shared_setup: "partly",
  org_model_keys: "built",
  org_github: "no",
  org_bot_triggers: "no",
  chat_handover: "partly",
  org_roles: "built",
  org_policy: "built",
};

/** Free's features are everyone's, so the catalog does not list them. */
const FREE: [string, BuiltState][] = [
  ["The whole app on Mac and Linux", "built"],
  ["Every harness and every model: frontier, coding plans, local", "built"],
  ["Unlimited agents, from different vendors on one task", "built"],
  [
    "Limits for every plan you pay for, and a chat that carries on in another agent when one runs out",
    "partly",
  ],
  [
    "@Chief per project: calls only when you must decide, Overrule and Undo, why with sources, rules said twice become checks",
    "built",
  ],
  ["Auto, line attribution", "built"],
  ["Git, browser, plugins, notch, dictation", "built"],
  [
    "Every computer you own, linked on one account and reachable from desktop and phone: your laptop, a headless box, one click into your own Hetzner or DigitalOcean",
    "partly",
  ],
  ["Your vault, on every host", "built"],
];

const TEAM_LATER = ["Review check on PRs", "Pin-to-agent widget"];

const TAGS: Record<string, string> = {
  free: "The distribution",
  team: "What a company pays for",
};

export function PricingPage({
  orgs,
  reload,
}: {
  orgs: Org[];
  reload: () => Promise<void>;
}) {
  const client = useClient();
  const [shedInterval, setShedInterval] = useState<BillingInterval>("month");
  const loaded = useLoad(
    async () => (await client("see the prices")).getBillingCatalog(),
    "",
  );
  const catalog = loaded.data;

  return (
    <Page
      title="Pricing"
      lede="What's sold, and what's built. Staff only."
      mark={iconMark("card")}
      scene="you"
      tone={loaded.problem ? "failed" : undefined}
    >
      {loaded.problem ? (
        <>
          <Nothing
            title="Prices didn't load"
            actions={
              <Btn quiet onClick={loaded.again}>
                <IconRefresh size={14} />
                Try again
              </Btn>
            }
          >
            We couldn&apos;t reach Sikemux. Nothing was changed.
          </Nothing>
          <Problem>{loaded.problem}</Problem>
        </>
      ) : !catalog ? (
        <Section label="Plans" busy />
      ) : (
        <>
          <Plans catalog={catalog} />
          <Shed
            catalog={catalog}
            interval={shedInterval}
            setInterval={setShedInterval}
          />
          <TeamTests catalog={catalog} orgs={orgs} reload={reload} />
        </>
      )}
    </Page>
  );
}

function Plans({ catalog }: { catalog: BillingCatalog }) {
  return (
    <Section>
      <div className="price-cols">
        {catalog.plans.map((plan) => {
          const product = catalog.products.find(
            (each) => each.kind === "plan" && each.plan === plan.id,
          );
          const { before, added } = featuresAdded(catalog, plan.id);
          const lines: CardLine[] =
            plan.id === "free"
              ? FREE.map(([text, state]) => ({ key: text, text, state }))
              : [
                  ...added.map((feature) => ({
                    key: feature,
                    text: FEATURE_WORDS[feature],
                    state: BUILT[feature],
                  })),
                  ...(plan.id === "team"
                    ? TEAM_LATER.map((text) => ({
                        key: text,
                        text: `Later: ${text}`,
                        state: "no" as const,
                      }))
                    : []),
                ];
          const { price, summary } = product
            ? planPricing(product)
            : {
                price: { amount: 0, currency: "usd" },
                summary: TAGS[plan.id],
              };
          return (
            <PlanCard
              key={plan.id}
              name={plan.name}
              price={price}
              summary={summary}
              plus={before ? `Everything in ${before}, plus` : undefined}
              lines={lines}
            />
          );
        })}
      </div>
      <p className="price-leg">
        <span className="built">
          <IconCheck size={14} />
          Built
        </span>
        <span className="partly">
          <IconMinus size={14} />
          Partly
        </span>
        <span className="no">
          <IconClock size={14} />
          Not built
        </span>
      </p>
    </Section>
  );
}

function Shed({
  catalog,
  interval,
  setInterval,
}: {
  catalog: BillingCatalog;
  interval: BillingInterval;
  setInterval: (interval: BillingInterval) => void;
}) {
  const boxes = catalog.products.filter((product) => product.kind === "shed");
  if (boxes.length === 0) return null;
  return (
    <Section
      label="Shed · add-on · any plan, Free included"
      aside={
        <BilledSwitch
          interval={interval}
          onChange={setInterval}
          saving={boxes[0]}
        />
      }
    >
      <p className="note">
        A Linux box we run, joined to your account. Buy one or several by the
        box. You sign in to Claude Code and Codex on it with your own plans.
      </p>
      <div className="price-cols">
        {boxes.map((product) => {
          const words = SHED_WORDS[product.id];
          const { price, summary } = priced(product, interval);
          return (
            <PlanCard
              key={product.id}
              name={product.name}
              price={price}
              summary={[summary, words?.tag].filter(Boolean).join(" · ")}
              lines={(words?.specs ?? []).map((text) => ({
                key: text,
                text,
                state: "no",
              }))}
            >
              {words?.runsOn}
            </PlanCard>
          );
        })}
      </div>
    </Section>
  );
}

function planPricing(product: BillingProduct) {
  const month = priceOf(product, "month");
  const year = priceOf(product, "year");
  const unit = UNIT_WORD[product.unit];
  if (month) {
    const per = [unit, PER.month].filter(Boolean).join(", ");
    const alt = year ? `, or ${money(year, { cents: false })} ${PER.year}` : "";
    return { price: month, summary: `${per}${alt}` };
  }
  return priced(product, "year");
}

/** The price at the chosen interval, else the one it is sold by, and the other interval's price. */
function priced(product: BillingProduct, interval: BillingInterval) {
  const shown = priceOf(product, interval) ? interval : "month";
  const other: BillingInterval = shown === "month" ? "year" : "month";
  const otherPrice = priceOf(product, other);
  const per = [UNIT_WORD[product.unit], PER[shown]].filter(Boolean).join(" ");
  const alt = otherPrice
    ? `, or ${money(otherPrice, { cents: false })} ${PER[other]}`
    : "";
  return {
    price: priceOf(product, shown),
    summary: `${per}${alt}`,
  };
}

/** Billing's tests: the checkout nobody in the free preview meets, and taking an org off it. */
function TeamTests({
  catalog,
  orgs,
  reload,
}: {
  catalog: BillingCatalog;
  orgs: Org[];
  reload: () => Promise<void>;
}) {
  const checkout = useCheckout();
  const say = useToast();
  const [making, setMaking] = useState(false);
  const bought = orgs.filter((org) => org.role === "owner" && !org.preview);

  return (
    <Section label="Team tests">
      <Rows>
        {bought.map((org) => (
          <MoveToPreview
            key={org.id}
            org={org}
            onMoved={(moved) => {
              say(`${moved.name} is in the free preview`);
              void reload();
            }}
          />
        ))}
        <Row
          name="Make a test org"
          detail="Makes an org and buys it a Team plan with a test card."
          end={
            <Btn
              small
              quiet
              onClick={() => {
                checkout.clear();
                setMaking(true);
              }}
            >
              Make one
            </Btn>
          }
        />
      </Rows>
      {making ? (
        <MakeTestOrgDialog
          catalog={catalog}
          checkout={checkout}
          onClose={() => setMaking(false)}
        />
      ) : null}
    </Section>
  );
}

function MakeTestOrgDialog({
  catalog,
  checkout,
  onClose,
}: {
  catalog: BillingCatalog;
  checkout: ReturnType<typeof useCheckout>;
  onClose: () => void;
}) {
  const [name, setName] = useState("");
  const [seats, setSeats] = useState(3);
  const team = productOf(catalog, "team");
  const unit = priceOf(team, "month");
  const trimmed = name.trim();

  return (
    <Dialog
      title="Make a test org"
      busy={checkout.busy}
      onClose={onClose}
      onSubmit={() => {
        if (!trimmed) return;
        void checkout.go(async (routes) => {
          const org = await routes.createOrg({ name: trimmed });
          return routes.startCheckout({
            product: "team",
            interval: "month",
            quantity: seats,
            orgId: org.id,
          });
        });
      }}
      foot={
        <>
          <Btn quiet onClick={onClose} disabled={checkout.busy}>
            Cancel
          </Btn>
          <Btn type="submit" busy={checkout.busy} disabled={!trimmed}>
            Go to checkout
          </Btn>
        </>
      }
    >
      <label className="dlg-field">
        <span>Org name</span>
        <Field
          placeholder="Test org"
          value={name}
          onChange={(event) => setName(event.target.value)}
          autoFocus
          maxLength={64}
        />
      </label>
      <div className="step-line">
        <Stepper
          value={seats}
          min={1}
          max={team?.maxQuantity ?? 1000}
          onChange={setSeats}
        />
        <span className="soft">
          seats
          {unit
            ? ` · ${money(times(unit, seats), { cents: false })} ${PER.month}`
            : ""}
        </span>
      </div>
      <Problem>{checkout.problem}</Problem>
    </Dialog>
  );
}

function MoveToPreview({
  org,
  onMoved,
}: {
  org: Org;
  onMoved: (org: Org) => void;
}) {
  const client = useClient();
  const action = useAction();
  return (
    <Row
      lead={<OrgAvatar org={org} size={28} />}
      name={org.name}
      detail={`Team, ${org.seats.limit} ${org.seats.limit === 1 ? "seat" : "seats"} · keeps the org and everything in it, and ends its test plan.`}
      end={
        <Btn
          small
          quiet
          busy={action.busy}
          onClick={() =>
            void action.run(
              async () => {
                const routes = await client("move your org");
                onMoved(await routes.moveOrgToPreview(org.id));
              },
              (error) => `${problemText(error)} ${org.name} is unchanged.`,
            )
          }
        >
          Move to the free preview
        </Btn>
      }
    >
      {action.problem ? <Problem>{action.problem}</Problem> : null}
    </Row>
  );
}
