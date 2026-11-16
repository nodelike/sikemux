import { useAuth } from "@clerk/react";
import type {
  BillingCatalog,
  BillingInterval,
  BillingOverview,
  Invoice,
  Money,
  Org,
  Subscription,
  SubscriptionChange,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import {
  IconCircleAlert,
  IconCreditCard,
  IconDownload,
  IconUndo,
} from "@shared/ui/Icons";
import { useCallback, useEffect, useState, type ReactNode } from "react";

import { api, problemText, signedInToken } from "./api.ts";
import { PlanCard } from "./PlanCard.tsx";
import {
  FEATURE_WORDS,
  featuresAdded,
  money,
  PER,
  priceOf,
  priceText,
  productOf,
  times,
  UNIT_WORD,
} from "./money.ts";
import {
  Confirm,
  ItemRow,
  PageHead,
  Row,
  Rows,
  Section,
  Segmented,
  useAction,
  useNotice,
} from "./ui.tsx";

const NOT_OPEN = "Buying is not open yet";
const ME = "me";

type Loaded = {
  catalog: BillingCatalog;
  overview: BillingOverview;
  invoices: Invoice[];
  /** When it was read, which decides whether a refund is still open. */
  readAt: number;
};

type Load =
  | { state: "loading" }
  | { state: "failed"; problem: string }
  | ({ state: "loaded" } & Loaded);

export function useClient() {
  const { getToken } = useAuth();
  return useCallback(
    async (what: string) => api(await signedInToken(getToken, what)),
    [getToken],
  );
}

/** Whether billing answers this account: it does for staff, and for everyone once it is live. */
export function useBillingShown(ready: boolean): boolean {
  const client = useClient();
  const [shown, setShown] = useState(false);
  useEffect(() => {
    if (!ready) return;
    let current = true;
    void client("see your plan")
      .then((routes) => routes.getBilling())
      .then(
        () => current && setShown(true),
        () => undefined,
      );
    return () => {
      current = false;
    };
  }, [ready, client]);
  return shown;
}

export function BillingPage({ version }: { version: number }) {
  const client = useClient();
  const [orgs, setOrgs] = useState<Org[]>();
  const [chosen, setChosen] = useState<string>();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [tick, setTick] = useState(0);
  const { notice, show } = useNotice();
  const owned = (orgs ?? []).filter((org) => org.role === "owner");
  const payer = chosen ?? owned[0]?.id ?? (orgs ? ME : undefined);
  const org = owned.find((each) => each.id === payer);
  const reload = () => setTick((n) => n + 1);

  useEffect(() => {
    let current = true;
    void client("see your plan")
      .then((routes) => routes.listOrgs())
      .then(
        ({ orgs }) => current && setOrgs(orgs),
        () => current && setOrgs([]),
      );
    return () => {
      current = false;
    };
  }, [version, client]);

  useEffect(() => {
    if (!payer) return;
    let current = true;
    const query = payer === ME ? {} : { org: payer };
    void (async () => {
      try {
        const routes = await client("see your plan");
        const [catalog, overview, { invoices }] = await Promise.all([
          routes.getBillingCatalog(),
          routes.getBilling(query),
          routes.listInvoices(query),
        ]);
        if (current)
          setLoad({
            state: "loaded",
            catalog,
            overview,
            invoices,
            readAt: Date.now(),
          });
      } catch (error) {
        if (current) setLoad({ state: "failed", problem: problemText(error) });
      }
    })();
    return () => {
      current = false;
    };
  }, [payer, version, tick, client]);

  const switcher =
    owned.length > 0 ? (
      <div className="payer">
        <Segmented
          label="Whose bill"
          value={payer ?? ME}
          onChange={(value) => {
            setLoad({ state: "loading" });
            setChosen(value);
          }}
          options={[
            { value: ME, name: "You" },
            ...owned.map((each) => ({ value: each.id, name: each.name })),
          ]}
        />
      </div>
    ) : null;

  if (load.state !== "loaded")
    return (
      <>
        <PageHead title="Plan and billing" />
        {switcher}
        {load.state === "failed" ? (
          <p className="problem" role="alert">
            {load.problem}
          </p>
        ) : (
          <div className="groups">
            <Section label="Plan">
              <Rows busy />
            </Section>
          </div>
        )}
      </>
    );

  return (
    <>
      <Bill
        {...load}
        org={org}
        orgs={orgs ?? []}
        switcher={switcher}
        reload={reload}
        show={show}
      />
      {notice}
    </>
  );
}

type Show = (text: string, undo?: () => Promise<void>) => void;

function Bill({
  catalog,
  overview,
  invoices,
  readAt,
  org,
  orgs,
  switcher,
  reload,
  show,
}: Loaded & {
  org?: Org;
  orgs: Org[];
  switcher: ReactNode;
  reload: () => void;
  show: Show;
}) {
  const { allowance, live, canBuy } = overview;
  const open = overview.subscriptions.filter((sub) => sub.status !== "ended");
  const kindOf = (sub: Subscription) => productOf(catalog, sub.product)?.kind;
  const planSub = open.find((sub) => kindOf(sub) === "plan");
  const shedSubs = open.filter((sub) => kindOf(sub) === "shed");
  const planName =
    catalog.plans.find((plan) => plan.id === allowance.plan)?.name ??
    allowance.plan;
  const seatIn = orgs
    .filter((each) => allowance.seatOrgs?.includes(each.id))
    .map((each) => each.name);
  const free = !org && allowance.plan === "free";
  const query = org ? { org: org.id } : {};

  const lede = org
    ? planSub
      ? `${org.name} is on ${planName}. You own the bill.`
      : `${org.name} has no Team plan, so nobody can be invited to it.`
    : free
      ? "You're on Free. It has no cap on hosts and never runs out."
      : !planSub && seatIn.length > 0
        ? `You have ${planName} through a seat in ${seatIn.join(", ")}.`
        : `You're on ${planName}.`;

  return (
    <>
      <PageHead title="Plan and billing">{lede}</PageHead>
      {switcher}
      <div className="groups">
        {planSub?.status === "lapsed" ? (
          <Lapsed sub={planSub} catalog={catalog} query={query} />
        ) : null}
        <Section
          label="Plan"
          aside={live ? null : <span className="testmode">TEST MODE</span>}
        >
          <Rows>
            {planSub ? (
              <PlanRows
                sub={planSub}
                catalog={catalog}
                canBuy={canBuy}
                reload={reload}
                show={show}
              />
            ) : (
              <Row label="Plan">
                <span className="chip">{org ? "No plan" : planName}</span>
                {free ? (
                  <>
                    <span>$0</span>
                    <span className="quiet">
                      · the whole app, every host you own
                    </span>
                  </>
                ) : seatIn.length > 0 && !org ? (
                  <span className="quiet">· through a seat</span>
                ) : null}
              </Row>
            )}
          </Rows>
        </Section>
        {free ? (
          <Upgrade catalog={catalog} canBuy={canBuy} />
        ) : org && !planSub ? (
          <StartTeam catalog={catalog} canBuy={canBuy} org={org} />
        ) : null}
        <NextCharge subs={open} catalog={catalog} card={overview.card} />
        {shedSubs.length > 0 ? (
          <Section label="Shed billing">
            <Rows>
              {shedSubs.map((sub) => (
                <ShedInterval
                  key={sub.id}
                  sub={sub}
                  catalog={catalog}
                  canBuy={canBuy}
                  reload={reload}
                  show={show}
                />
              ))}
            </Rows>
          </Section>
        ) : null}
        <Refunds
          invoices={invoices}
          readAt={readAt}
          catalog={catalog}
          reload={reload}
          show={show}
        />
        {open.length === 0 && invoices.length === 0 ? (
          <Section label="Billing">
            <Rows>
              <Row label="Payment method">
                <span className="quiet">
                  No card yet. You add one when you upgrade.
                </span>
              </Row>
              <Row label="Invoices">
                <span className="quiet">None yet</span>
              </Row>
              <Row label="Refunds">
                <span className="quiet">
                  No free trial: a first payment is refunded in full if you ask
                  within {catalog.refundDays} days.
                </span>
              </Row>
            </Rows>
          </Section>
        ) : (
          <>
            <PaymentMethod card={overview.card} query={query} />
            <Invoices invoices={invoices} catalog={catalog} />
          </>
        )}
      </div>
    </>
  );
}

function usePortal(query: { org?: string }) {
  const client = useClient();
  const action = useAction();
  const open = () =>
    action.run(
      async () => {
        const { url } = await (
          await client("change your card")
        ).getBillingPortal(query);
        location.assign(url);
      },
      (error) => `${problemText(error)} Your card is unchanged.`,
    );
  return { ...action, open };
}

function Lapsed({
  sub,
  catalog,
  query,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  query: { org?: string };
}) {
  const portal = usePortal(query);
  const name = productOf(catalog, sub.product)?.name ?? sub.product;
  return (
    <div className="banner warn" role="alert">
      <IconCircleAlert size={16} />
      <div className="grow">
        <b>{name} has lapsed: its last payment didn't go through.</b>
        <br />
        {sub.product === "team"
          ? "Invites and org features are frozen until it's paid. Everyone keeps their own hosts."
          : "Its features are off until it's paid. Your hosts keep working."}
        {portal.problem ? <p className="problem">{portal.problem}</p> : null}
      </div>
      <button
        type="button"
        className="button small primary"
        onClick={() => void portal.open()}
        disabled={portal.busy}
        data-busy={portal.busy || undefined}
      >
        Update card
      </button>
    </div>
  );
}

function intervalWord(interval: BillingInterval) {
  return interval === "month" ? "monthly" : "yearly";
}

function PlanRows({
  sub,
  catalog,
  canBuy,
  reload,
  show,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  canBuy: boolean;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const product = productOf(catalog, sub.product);
  const name = product?.name ?? sub.product;
  const seats = product?.unit === "seat";
  const ends = formatDay(sub.currentPeriodEnd, "long");
  const [asking, setAsking] = useState<"seats" | "cancel">();
  const [count, setCount] = useState(sub.quantity);
  const action = useAction();
  const resume = async () => {
    await (await client("keep your plan")).resumeSubscription(sub.id);
    reload();
  };
  const close = () => {
    setAsking(undefined);
    action.clear();
  };
  const change = (body: SubscriptionChange, done: string) =>
    action.run(
      async () => {
        await (
          await client("change your plan")
        ).changeSubscription(sub.id, body);
        close();
        reload();
        show(done);
      },
      (error) => `${problemText(error)} Nothing was charged.`,
    );
  const price = product ? priceText(product, sub.interval) : undefined;
  const changing =
    sub.atRenewal &&
    (sub.atRenewal.quantity !== sub.quantity ||
      sub.atRenewal.interval !== sub.interval ||
      sub.atRenewal.product !== sub.product)
      ? sub.atRenewal
      : undefined;

  return (
    <>
      <Row
        label="Plan"
        action={
          seats ? (
            <button
              type="button"
              className="button small"
              onClick={() =>
                asking === "seats" ? close() : setAsking("seats")
              }
              disabled={!canBuy}
              title={canBuy ? undefined : NOT_OPEN}
              aria-expanded={asking === "seats"}
            >
              {canBuy ? "Change seats" : NOT_OPEN}
            </button>
          ) : null
        }
        below={
          asking === "seats" ? (
            <Confirm
              problem={action.problem}
              busy={action.busy}
              label={
                count > sub.quantity
                  ? `Add ${count - sub.quantity} ${count - sub.quantity === 1 ? "seat" : "seats"}`
                  : `Go to ${count} ${count === 1 ? "seat" : "seats"}`
              }
              onCancel={close}
              onConfirm={() =>
                void change(
                  { quantity: count },
                  count > sub.quantity
                    ? `${count - sub.quantity} ${count - sub.quantity === 1 ? "seat" : "seats"} added`
                    : `${name} is now ${count} ${count === 1 ? "seat" : "seats"}`,
                )
              }
            >
              <label className="stepper-line">
                <Stepper
                  value={count}
                  min={1}
                  max={product?.maxQuantity ?? 1000}
                  onChange={setCount}
                />
                <span className="quiet">
                  seats. Added seats are charged now for what's left of this
                  period; fewer can't go below the people seated.
                </span>
              </label>
            </Confirm>
          ) : null
        }
      >
        <span className="chip accent">{name}</span>
        {seats ? (
          <span>
            {sub.quantity} {sub.quantity === 1 ? "seat" : "seats"}
          </span>
        ) : null}
        {price ? <span className="quiet">· {price}</span> : null}
      </Row>
      {sub.product === "team" ? (
        <Row label="Includes">
          <span>Pro on your own hosts while you hold a seat</span>
        </Row>
      ) : null}
      {changing ? (
        <Row label="At renewal">
          <span>
            {[
              changing.product !== sub.product
                ? productOf(catalog, changing.product)?.name
                : undefined,
              changing.quantity !== sub.quantity && seats
                ? `${changing.quantity} seats`
                : undefined,
              changing.interval !== sub.interval
                ? intervalWord(changing.interval)
                : undefined,
            ]
              .filter(Boolean)
              .join(", ")}
          </span>
          <span className="quiet">· from {ends}</span>
        </Row>
      ) : null}
      {sub.cancelAtPeriodEnd ? (
        <Row
          label="Ends"
          action={
            <button
              type="button"
              className="button small primary"
              onClick={() =>
                void action.run(
                  async () => {
                    await resume();
                    show(`${name} renews on ${ends}.`);
                  },
                  (error) => `${problemText(error)} It still ends on ${ends}.`,
                )
              }
              disabled={action.busy}
              data-busy={action.busy || undefined}
            >
              Keep {name}
            </button>
          }
          below={
            action.problem && !asking ? (
              <p className="problem" role="alert">
                {action.problem}
              </p>
            ) : null
          }
        >
          <span>{ends}</span>
          {sub.product === "team" ? (
            <span className="quiet">
              · invites and org features freeze then
            </span>
          ) : null}
        </Row>
      ) : (
        <Row
          label="Renews"
          action={
            <button
              type="button"
              className="button small quiet-danger"
              onClick={() =>
                asking === "cancel" ? close() : setAsking("cancel")
              }
              aria-expanded={asking === "cancel"}
            >
              Cancel {name}
            </button>
          }
          below={
            asking === "cancel" ? (
              <Confirm
                problem={action.problem}
                busy={action.busy}
                label={`Cancel ${name}`}
                danger
                onCancel={close}
                onConfirm={() =>
                  void action.run(
                    async () => {
                      await (
                        await client("cancel your plan")
                      ).cancelSubscription(sub.id);
                      close();
                      reload();
                      show(`${name} cancelled. It runs until ${ends}.`, resume);
                    },
                    (error) => `${problemText(error)} It still renews.`,
                  )
                }
              >
                <p>
                  {name} runs until {ends}, then stops renewing.{" "}
                  {sub.product === "team"
                    ? "Invites and org features freeze then; everyone keeps their own hosts and the org keeps its data."
                    : "Your hosts and data stay as they are."}
                </p>
              </Confirm>
            ) : null
          }
        >
          <span>{ends}</span>
          <span className="quiet">· {intervalWord(sub.interval)}</span>
        </Row>
      )}
    </>
  );
}

function Stepper({
  value,
  min,
  max,
  onChange,
}: {
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
}) {
  return (
    <span className="stepper">
      <button
        type="button"
        aria-label="Fewer"
        onClick={() => onChange(Math.max(min, value - 1))}
        disabled={value <= min}
      >
        −
      </button>
      <input
        inputMode="numeric"
        aria-label="How many"
        value={value}
        onChange={(event) => {
          const next = Number.parseInt(event.target.value, 10);
          if (Number.isFinite(next))
            onChange(Math.min(max, Math.max(min, next)));
        }}
      />
      <button
        type="button"
        aria-label="More"
        onClick={() => onChange(Math.min(max, value + 1))}
        disabled={value >= max}
      >
        +
      </button>
    </span>
  );
}

function useCheckout() {
  const client = useClient();
  const action = useAction();
  const go = (
    start: (
      routes: Awaited<ReturnType<typeof client>>,
    ) => Promise<{ url: string }>,
  ) =>
    action.run(
      async () => {
        const { url } = await start(await client("buy a plan"));
        location.assign(url);
      },
      (error) => `${problemText(error)} Nothing was charged.`,
    );
  return { ...action, go };
}

function Upgrade({
  catalog,
  canBuy,
}: {
  catalog: BillingCatalog;
  canBuy: boolean;
}) {
  const [interval, setInterval] = useState<BillingInterval>("month");
  const pro = productOf(catalog, "pro");
  const team = productOf(catalog, "team");
  const checkout = useCheckout();
  const [naming, setNaming] = useState(false);
  const proMonth = priceOf(pro, "month");
  const proYear = priceOf(pro, "year");
  const monthsFree =
    proMonth && proYear
      ? Math.round((proMonth.amount * 12 - proYear.amount) / proMonth.amount)
      : 0;

  return (
    <Section
      label="Upgrade"
      aside={
        <Segmented
          label="Billed"
          value={interval}
          onChange={setInterval}
          options={[
            { value: "month", name: "Monthly" },
            {
              value: "year",
              name: (
                <>
                  Yearly
                  {monthsFree > 0 ? (
                    <em className="save">{monthsFree} months free</em>
                  ) : null}
                </>
              ),
            },
          ]}
        />
      }
    >
      <div className="plan-cards">
        {pro ? (
          <PlanOffer
            catalog={catalog}
            plan="pro"
            price={priceOf(pro, interval) ?? proMonth}
            per={priceOf(pro, interval) ? PER[interval] : PER.month}
            other={
              interval === "month" && proYear
                ? `or ${money(proYear, { cents: false })} a year`
                : proMonth
                  ? `or ${money(proMonth, { cents: false })} a month`
                  : undefined
            }
          >
            {checkout.problem && !naming ? (
              <p className="problem" role="alert">
                {checkout.problem}
              </p>
            ) : null}
            <button
              type="button"
              className="button primary"
              disabled={!canBuy || checkout.busy}
              data-busy={(checkout.busy && !naming) || undefined}
              onClick={() =>
                void checkout.go((routes) =>
                  routes.startCheckout({
                    product: "pro",
                    interval,
                    quantity: 1,
                  }),
                )
              }
            >
              {canBuy ? "Upgrade to Pro" : NOT_OPEN}
            </button>
          </PlanOffer>
        ) : null}
        {team ? (
          <PlanOffer
            catalog={catalog}
            plan="team"
            price={priceOf(team, "month")}
            per={`${UNIT_WORD.seat} ${PER.month}`}
          >
            {naming ? (
              <NewTeam
                catalog={catalog}
                checkout={checkout}
                onCancel={() => setNaming(false)}
              />
            ) : (
              <button
                type="button"
                className="button"
                disabled={!canBuy}
                onClick={() => {
                  checkout.clear();
                  setNaming(true);
                }}
              >
                {canBuy ? "Start Team" : NOT_OPEN}
              </button>
            )}
          </PlanOffer>
        ) : null}
      </div>
    </Section>
  );
}

function PlanOffer({
  catalog,
  plan,
  price,
  per,
  other,
  children,
}: {
  catalog: BillingCatalog;
  plan: "pro" | "team";
  price?: Money;
  per: string;
  other?: string;
  children: ReactNode;
}) {
  const { before, added } = featuresAdded(catalog, plan);
  return (
    <PlanCard
      name={catalog.plans.find((each) => each.id === plan)?.name ?? plan}
      price={price}
      per={other ? `${per} · ${other}` : per}
      plus={before ? `Everything in ${before}, plus` : undefined}
      lines={added.map((feature) => ({
        key: feature,
        text: FEATURE_WORDS[feature],
      }))}
    >
      {children}
    </PlanCard>
  );
}

function NewTeam({
  catalog,
  checkout,
  onCancel,
}: {
  catalog: BillingCatalog;
  checkout: ReturnType<typeof useCheckout>;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [seats, setSeats] = useState(3);
  const team = productOf(catalog, "team");
  const unit = priceOf(team, "month");
  const start = () =>
    void checkout.go(async (routes) => {
      const org = await routes.createOrg({ name: name.trim() });
      return routes.startCheckout({
        product: "team",
        interval: "month",
        quantity: seats,
        orgId: org.id,
      });
    });
  return (
    <form
      className="new-team"
      onSubmit={(event) => {
        event.preventDefault();
        if (name.trim()) start();
      }}
    >
      <input
        className="field"
        placeholder="Org name"
        aria-label="Org name"
        value={name}
        onChange={(event) => setName(event.target.value)}
        autoFocus
        maxLength={80}
      />
      <label className="stepper-line">
        <Stepper
          value={seats}
          min={1}
          max={team?.maxQuantity ?? 1000}
          onChange={setSeats}
        />
        <span className="quiet">
          seats
          {unit
            ? ` · ${money(times(unit, seats), { cents: false })} a month`
            : ""}
        </span>
      </label>
      {checkout.problem ? (
        <p className="problem" role="alert">
          {checkout.problem}
        </p>
      ) : null}
      <div className="actions">
        <button
          type="button"
          className="button small"
          onClick={onCancel}
          disabled={checkout.busy}
        >
          Cancel
        </button>
        <button
          type="submit"
          className="button small primary"
          disabled={!name.trim() || checkout.busy}
          data-busy={checkout.busy || undefined}
        >
          Start Team
        </button>
      </div>
    </form>
  );
}

function StartTeam({
  catalog,
  canBuy,
  org,
}: {
  catalog: BillingCatalog;
  canBuy: boolean;
  org: Org;
}) {
  const team = productOf(catalog, "team");
  const [seats, setSeats] = useState(Math.max(1, org.seats.held));
  const checkout = useCheckout();
  const unit = priceOf(team, "month");
  return (
    <Section label="Team plan">
      <Rows>
        <Row
          label="Seats"
          action={
            <button
              type="button"
              className="button small primary"
              disabled={!canBuy || checkout.busy}
              data-busy={checkout.busy || undefined}
              onClick={() =>
                void checkout.go((routes) =>
                  routes.startCheckout({
                    product: "team",
                    interval: "month",
                    quantity: seats,
                    orgId: org.id,
                  }),
                )
              }
            >
              {canBuy ? "Start Team" : NOT_OPEN}
            </button>
          }
          below={
            checkout.problem ? (
              <p className="problem" role="alert">
                {checkout.problem}
              </p>
            ) : null
          }
        >
          <Stepper
            value={seats}
            min={Math.max(1, org.seats.held)}
            max={team?.maxQuantity ?? 1000}
            onChange={setSeats}
          />
          {unit ? (
            <span className="quiet">
              · {money(times(unit, seats), { cents: false })} a month
            </span>
          ) : null}
        </Row>
      </Rows>
    </Section>
  );
}

function NextCharge({
  subs,
  catalog,
  card,
}: {
  subs: Subscription[];
  catalog: BillingCatalog;
  card?: BillingOverview["card"];
}) {
  const renewing = subs
    .filter((sub) => sub.status === "active" && !sub.cancelAtPeriodEnd)
    .flatMap((sub) => {
      const next = sub.atRenewal ?? sub;
      const product = productOf(catalog, next.product);
      const unit = priceOf(product, next.interval);
      return unit ? [{ sub, next, product, unit }] : [];
    })
    .sort((a, b) =>
      a.sub.currentPeriodEnd.localeCompare(b.sub.currentPeriodEnd),
    );
  if (renewing.length === 0) return null;
  const total = renewing.reduce(
    (sum, each) => sum + each.unit.amount * each.next.quantity,
    0,
  );
  const currency = renewing[0]?.unit.currency ?? "usd";

  return (
    <Section label="Next charge">
      <Rows>
        {renewing.map(({ sub, next, product, unit }, index) => {
          const day = formatDay(sub.currentPeriodEnd, "long");
          const shownDay =
            index === 0 ||
            renewing[index - 1]?.sub.currentPeriodEnd !== sub.currentPeriodEnd;
          const what =
            product?.unit === "seat"
              ? `${product.name}, ${next.quantity} seats × ${money(unit, { cents: false })}`
              : product?.unit === "box"
                ? `${product.name}, ${next.quantity} ${next.quantity === 1 ? "box" : "boxes"}, ${intervalWord(next.interval)}`
                : `${product?.name ?? next.product}, ${intervalWord(next.interval)}`;
          return (
            <Row
              key={sub.id}
              label={shownDay ? day : ""}
              action={
                <span className="amount">
                  {money(times(unit, next.quantity))}
                </span>
              }
            >
              <span className="quiet">{what}</span>
            </Row>
          );
        })}
        {renewing.length > 1 ? (
          <Row
            label="Total"
            action={
              <span className="amount big">
                {money({ amount: total, currency })}
              </span>
            }
          >
            {card ? <span className="quiet">{cardName(card)}</span> : null}
          </Row>
        ) : null}
      </Rows>
    </Section>
  );
}

function ShedInterval({
  sub,
  catalog,
  canBuy,
  reload,
  show,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  canBuy: boolean;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const product = productOf(catalog, sub.product);
  const action = useAction();
  const chosen = sub.atRenewal?.interval ?? sub.interval;
  const name = product?.name ?? sub.product;
  const option = (interval: BillingInterval) => {
    const unit = priceOf(product, interval);
    return {
      value: interval,
      name: `${interval === "month" ? "Monthly" : "Yearly"}${unit ? ` · ${money(unit, { cents: false })}` : ""}`,
    };
  };
  const pick = (interval: BillingInterval) => {
    if (interval === chosen || action.busy || !canBuy) return;
    void action.run(
      async () => {
        await (
          await client("change how your box is billed")
        ).changeSubscription(sub.id, { interval, when: "renewal" });
        reload();
        show(
          `${name} goes ${intervalWord(interval)} on ${formatDay(sub.currentPeriodEnd, "long")}.`,
        );
      },
      (error) => `${problemText(error)} It stays ${intervalWord(chosen)}.`,
    );
  };
  return (
    <Row
      label={name}
      below={
        action.problem ? (
          <p className="problem" role="alert">
            {action.problem}
          </p>
        ) : null
      }
    >
      <span data-busy={action.busy || undefined}>
        <Segmented
          label={`How ${name} is billed`}
          value={chosen}
          onChange={pick}
          options={(["month", "year"] as const)
            .filter((interval) => priceOf(product, interval))
            .map(option)}
        />
      </span>
      {sub.quantity > 1 ? (
        <span className="quiet">· {sub.quantity} boxes</span>
      ) : null}
    </Row>
  );
}

function invoiceWhat(invoice: Invoice, catalog: BillingCatalog): string {
  return invoice.lines
    .map((line) => {
      const product = productOf(catalog, line.product);
      const name = product?.name ?? line.product;
      if (product?.unit === "seat") return `${name}, ${line.quantity} seats`;
      if (product?.unit === "box" && line.quantity > 1)
        return `${name}, ${line.quantity} boxes`;
      return name;
    })
    .join(" · ");
}

function Refunds({
  invoices,
  readAt,
  catalog,
  reload,
  show,
}: {
  invoices: Invoice[];
  readAt: number;
  catalog: BillingCatalog;
  reload: () => void;
  show: Show;
}) {
  const open = invoices.filter(
    (invoice) =>
      invoice.status === "paid" &&
      (invoice.refundRequestedAt ||
        (invoice.refundableUntil &&
          Date.parse(invoice.refundableUntil) > readAt)),
  );
  if (open.length === 0) return null;
  return (
    <Section label="Refund">
      <Rows>
        {open.map((invoice) => (
          <RefundRow
            key={invoice.id}
            invoice={invoice}
            catalog={catalog}
            reload={reload}
            show={show}
          />
        ))}
      </Rows>
    </Section>
  );
}

function RefundRow({
  invoice,
  catalog,
  reload,
  show,
}: {
  invoice: Invoice;
  catalog: BillingCatalog;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const [asking, setAsking] = useState(false);
  const action = useAction();
  const first = invoice.lines[0]?.product;
  const plan = first ? (productOf(catalog, first)?.name ?? first) : "This plan";
  const bought = formatDay(invoice.at, "long");
  const name = `${plan}, bought ${bought}`;

  if (invoice.refundRequestedAt || !invoice.refundableUntil)
    return (
      <ItemRow
        glyph={<IconUndo size={16} />}
        name={name}
        detail={`Refund asked${invoice.refundRequestedAt ? ` on ${formatDay(invoice.refundRequestedAt, "long")}` : ""}. It takes a few days to come back, and the plan ends then.`}
      />
    );

  const until = formatDay(invoice.refundableUntil, "long");
  return (
    <ItemRow
      glyph={<IconUndo size={16} />}
      name={name}
      detail={`Everything paid on it in its first ${catalog.refundDays} days comes back if you ask by ${until}. It's given once.`}
      end={
        <button
          type="button"
          className="button small"
          onClick={() => {
            action.clear();
            setAsking(!asking);
          }}
          aria-expanded={asking}
        >
          Ask for a refund
        </button>
      }
    >
      {asking ? (
        <Confirm
          problem={action.problem}
          busy={action.busy}
          label="Ask for the refund"
          onCancel={() => setAsking(false)}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("ask for a refund")
                ).refundInvoice(invoice.id);
                setAsking(false);
                reload();
                show("Refund asked. It takes a few days to come back.");
              },
              (error) => `${problemText(error)} Nothing was refunded.`,
            )
          }
        >
          <p>
            Ask for a refund of {plan}? Everything paid on it since {bought}
            comes back. The payment provider takes a few days, and {plan} ends
            once it has. A refund is given once for each plan, so buying {plan}{" "}
            again has none.
          </p>
        </Confirm>
      ) : null}
    </ItemRow>
  );
}

function cardName(card: NonNullable<BillingOverview["card"]>): string {
  const brand = card.brand.charAt(0).toUpperCase() + card.brand.slice(1);
  return `${brand} ending ${card.last4}`;
}

function PaymentMethod({
  card,
  query,
}: {
  card?: BillingOverview["card"];
  query: { org?: string };
}) {
  const portal = usePortal(query);
  const update = (
    <button
      type="button"
      className="button small"
      onClick={() => void portal.open()}
      disabled={portal.busy}
      data-busy={portal.busy || undefined}
    >
      {card ? "Update" : "Add a card"}
    </button>
  );
  return (
    <Section label="Payment method">
      <Rows>
        <ItemRow
          glyph={<IconCreditCard size={16} />}
          name={card ? cardName(card) : "No card on file"}
          detail={
            card
              ? `Expires ${String(card.expMonth).padStart(2, "0")}/${String(card.expYear).slice(-2)}`
              : "The payment provider holds the card; we never see it."
          }
          end={update}
        >
          {portal.problem ? (
            <p className="problem row-problem" role="alert">
              {portal.problem}
            </p>
          ) : null}
        </ItemRow>
      </Rows>
    </Section>
  );
}

const STATUS_WORDS: Record<Invoice["status"], string> = {
  paid: "Paid",
  failed: "Failed",
  refunded: "Refunded",
};

function Invoices({
  invoices,
  catalog,
}: {
  invoices: Invoice[];
  catalog: BillingCatalog;
}) {
  if (invoices.length === 0)
    return (
      <Section label="Invoices">
        <Rows>
          <Row label="Invoices">
            <span className="quiet">None yet</span>
          </Row>
        </Rows>
      </Section>
    );
  return (
    <Section label="Invoices" count={invoices.length}>
      <Rows>
        {invoices.map((invoice) => (
          <InvoiceRow key={invoice.id} invoice={invoice} catalog={catalog} />
        ))}
      </Rows>
    </Section>
  );
}

function InvoiceRow({
  invoice,
  catalog,
}: {
  invoice: Invoice;
  catalog: BillingCatalog;
}) {
  const client = useClient();
  const action = useAction();
  const receipt = () =>
    action.run(
      async () => {
        const { url } = await (
          await client("get the receipt")
        ).getInvoiceReceipt(invoice.id);
        location.assign(url);
      },
      (error) => `${problemText(error)} Try the receipt again.`,
    );
  return (
    <li className="row invoice">
      <span className="quiet">{formatDay(invoice.at, "short")}</span>
      <span className="ink">{invoiceWhat(invoice, catalog)}</span>
      <span className="amount">{money(invoice.total)}</span>
      <span className="status" data-status={invoice.status}>
        {STATUS_WORDS[invoice.status]}
      </span>
      <button
        type="button"
        className="icon-button"
        onClick={() => void receipt()}
        disabled={action.busy}
        data-busy={action.busy || undefined}
        aria-label="Download the receipt"
        title={action.problem ?? "Download the receipt"}
      >
        <IconDownload size={15} />
      </button>
    </li>
  );
}
