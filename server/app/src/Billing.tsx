import { useAuth } from "@clerk/react";
import type {
  BillingCatalog,
  BillingInterval,
  BillingOverview,
  BillingProductId,
  ChangePreview,
  ChangeWhen,
  Invoice,
  Money,
  Org,
  PlanFeature,
  ScheduledChange,
  SeatPause,
  Subscription,
  SubscriptionChange,
  TeamAccess,
} from "@sikemux/protocol";
import { formatDay } from "@shared/lib/format";
import {
  IconCircleAlert,
  IconCreditCard,
  IconDownload,
  IconInfo,
  IconUndo,
  IconUser,
  IconUsers,
} from "@shared/ui/Icons";
import {
  useCallback,
  useEffect,
  useState,
  type MouseEvent,
  type ReactNode,
} from "react";

import { api, problemText, signedInToken } from "./api.ts";
import { BilledSwitch } from "./BilledSwitch.tsx";
import { PlanCard } from "./PlanCard.tsx";
import { TEAM } from "./navigation.ts";
import { ShedBoxes } from "./Shed.tsx";
import {
  FEATURE_WORDS,
  featuresAdded,
  inPreview,
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
  PriceRow,
  Row,
  Rows,
  Section,
  Segmented,
  useAction,
  useNotice,
} from "./old.tsx";

export const FREE_DURING_PREVIEW = "Free during preview";
export const NOT_OPEN = "Buying is not open yet";
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

/** The org's own plan and bill, for its owners. */
export function OrgBillingPage({
  org,
}: {
  org: Org;
  version: number;
  team: TeamAccess;
}) {
  return <p>{org.name}</p>;
}

export function BillingPage({
  version,
  staff,
  team,
}: {
  version: number;
  staff: boolean;
  team: TeamAccess;
  orgs: Org[];
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
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
        staff={staff}
        team={team}
        reload={reload}
        show={show}
      />
      {notice}
    </>
  );
}

export type Show = (text: string, undo?: () => Promise<void>) => void;

function Bill({
  catalog,
  overview,
  invoices,
  readAt,
  org,
  orgs,
  switcher,
  staff,
  team,
  reload,
  show,
}: Loaded & {
  org?: Org;
  orgs: Org[];
  switcher: ReactNode;
  staff: boolean;
  team: TeamAccess;
  reload: () => void;
  show: Show;
}) {
  const { allowance, live, canBuy } = overview;
  const [interval, setInterval] = useState<BillingInterval>("month");
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
  const pays =
    overview.payer !== "another_owner" && overview.payer !== "departed";
  const ends = planSub ? formatDay(planSub.currentPeriodEnd, "long") : "";
  const giftEnds =
    !org && !planSub && allowance.giftEnds
      ? formatDay(allowance.giftEnds, "long")
      : "";

  const lede = org
    ? planSub
      ? overview.payer === "another_owner"
        ? `${org.name} is on ${planName}. Another owner pays for it, so only they can change it.`
        : overview.payer === "departed"
          ? `${org.name} is on ${planName} until ${ends}. Whoever paid for it no longer owns the org, so it ends then and can be bought again once it has.`
          : `${org.name} is on ${planName}. You own the bill.`
      : org.preview
        ? `${org.name} is in the Team preview. It is free, for up to ${org.seats.limit} people.`
        : `${org.name} has no Team plan, so nobody can be invited to it.`
    : free
      ? "You're on Free. It has no cap on hosts and never runs out."
      : !planSub && seatIn.length > 0
        ? `You have ${planName} through a seat in ${seatIn.join(", ")}.`
        : giftEnds
          ? `${planName} is yours free until ${giftEnds}. After that you're on Free, and nothing you have is deleted.`
          : `You're on ${planName}.`;

  const pro = productOf(catalog, "pro");
  const boxes = catalog.products.filter((product) => product.kind === "shed");
  const buyingBoxes = staff && !org && boxes.length > 0;
  const priced = free || buyingBoxes;

  return (
    <>
      <PageHead
        title="Plan and billing"
        tag={live ? null : <span className="testmode">TEST MODE</span>}
        aside={
          priced ? (
            <BilledSwitch
              interval={interval}
              onChange={setInterval}
              saving={free ? pro : boxes[0]}
            />
          ) : null
        }
      >
        {lede}
      </PageHead>
      {switcher}
      <div className="groups">
        {planSub?.status === "lapsed" ? (
          <Lapsed sub={planSub} catalog={catalog} query={query} pays={pays} />
        ) : null}
        <Section label="Plan">
          <Rows>
            {planSub ? (
              <PlanRows
                sub={planSub}
                catalog={catalog}
                canBuy={canBuy}
                pays={pays}
                reload={reload}
                show={show}
              />
            ) : (
              <PriceRow
                glyph={org ? <IconUsers size={16} /> : <IconUser size={16} />}
                name={
                  org ? (org.preview ? "Team preview" : "No plan") : planName
                }
                detail={
                  org?.preview
                    ? "Free during preview"
                    : free
                      ? "The whole app, on every host you own"
                      : seatIn.length > 0 && !org
                        ? `Through a seat in ${seatIn.join(", ")}`
                        : giftEnds
                          ? `Free until ${giftEnds}`
                          : undefined
                }
                price={free ? "$0" : undefined}
                per={free ? PER.month : undefined}
                end={org ? null : <span className="quiet">Your plan</span>}
              />
            )}
          </Rows>
        </Section>
        {free ? (
          <Upgrade
            catalog={catalog}
            canBuy={canBuy}
            interval={interval}
            access={team}
          />
        ) : org && !planSub && !org.preview ? (
          <StartTeam catalog={catalog} canBuy={canBuy} org={org} />
        ) : null}
        <ShedBoxes
          catalog={catalog}
          bought={shedSubs}
          buying={buyingBoxes}
          interval={interval}
          canBuy={canBuy}
          pays={pays}
          reload={reload}
          show={show}
        />
        <NextCharge subs={open} catalog={catalog} card={overview.card} />
        <Refunds
          subs={open}
          readAt={readAt}
          catalog={catalog}
          reload={reload}
          show={show}
        />
        {open.length === 0 && invoices.length === 0 ? (
          <Section label="Billing">
            <Rows>
              <ItemRow
                glyph={<IconCreditCard size={16} />}
                name="No card or invoices yet"
                detail="You add a card at your first checkout; each payment's invoice shows here."
              />
              <li className="row note">
                <span className="glyph">
                  <IconInfo size={15} />
                </span>
                <span>
                  Everything you pay for a plan in its first{" "}
                  {catalog.refundDays} days comes back in full if you ask within
                  them, once for each plan.
                </span>
              </li>
            </Rows>
          </Section>
        ) : (
          <>
            {pays ? <PaymentMethod card={overview.card} query={query} /> : null}
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
  pays,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  query: { org?: string };
  pays: boolean;
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
        {pays ? null : " Only whoever pays for it can update the card."}
        {portal.problem ? <p className="problem">{portal.problem}</p> : null}
      </div>
      {pays ? (
        <button
          type="button"
          className="button small primary"
          onClick={() => void portal.open()}
          disabled={portal.busy}
          data-busy={portal.busy || undefined}
        >
          Update card
        </button>
      ) : null}
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
  pays,
  reload,
  show,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  canBuy: boolean;
  pays: boolean;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const product = productOf(catalog, sub.product);
  const name = product?.name ?? sub.product;
  const seats = product?.unit === "seat";
  const ends = formatDay(sub.currentPeriodEnd, "long");
  const refunding = !!sub.refundRequestedAt;
  const [asking, setAsking] = useState<"seats" | "cancel">();
  const action = useAction();
  const resume = async () => {
    await (await client("keep your plan")).resumeSubscription(sub.id);
    reload();
  };
  const close = () => {
    setAsking(undefined);
    action.clear();
  };
  const price = product ? priceText(product, sub.interval) : undefined;
  const held = heldChange(sub);
  const pause = sub.seatPause;

  return (
    <>
      <Row
        label="Plan"
        action={
          seats && !refunding && pays && !held ? (
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
            <ChangeSeats
              sub={sub}
              name={name}
              max={product?.maxQuantity}
              onClose={close}
              onChanged={(text) => {
                reload();
                show(text);
              }}
            />
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
      <AtRenewal
        sub={sub}
        catalog={catalog}
        pays={pays}
        reload={reload}
        show={show}
      />
      {refunding ? (
        <Row label="Ends">
          <span>When its refund is paid</span>
          <span className="quiet">· it can't be changed until then</span>
        </Row>
      ) : sub.cancelAtPeriodEnd ? (
        <Row
          label="Ends"
          action={
            pays ? (
              <button
                type="button"
                className="button small primary"
                onClick={() =>
                  void action.run(
                    async () => {
                      await resume();
                      show(`${name} renews on ${ends}.`);
                    },
                    (error) =>
                      `${problemText(error)} It still ends on ${ends}.`,
                  )
                }
                disabled={action.busy}
                data-busy={action.busy || undefined}
              >
                Keep {name}
              </button>
            ) : null
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
          label={pause ? (pause.resumesAt ? "Resumes" : "Paused") : "Renews"}
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
                      if (pause)
                        show(
                          `${name} cancelled. The days you'd already paid are refunded.`,
                        );
                      else
                        show(
                          `${name} cancelled. It runs until ${ends}.`,
                          resume,
                        );
                    },
                    (error) =>
                      `${problemText(error)} ${pause ? "It stays paused." : "It still renews."}`,
                  )
                }
              >
                <p>
                  {pause
                    ? `${name} ends now, and the days you'd already paid are refunded. `
                    : `${name} runs until ${ends}, then stops renewing. `}
                  {sub.product === "team"
                    ? "Invites and org features freeze then; everyone keeps their own hosts and the org keeps its data."
                    : "Your hosts and data stay as they are."}
                </p>
              </Confirm>
            ) : null
          }
        >
          {pause ? (
            <span>{pausedLine(pause)}</span>
          ) : (
            <>
              <span>{ends}</span>
              <span className="quiet">· {intervalWord(sub.interval)}</span>
            </>
          )}
        </Row>
      )}
    </>
  );
}

/** What a Pro paused for a seat runs on, in one line. */
function pausedLine(pause: SeatPause) {
  const orgs = [...pause.orgs];
  const last = orgs.pop();
  if (last)
    return orgs.length
      ? `Paused while your seats in ${orgs.join(", ")} and ${last} include Pro.`
      : `Paused while your seat in ${last} includes Pro.`;
  return pause.resumesAt
    ? `Pro until ${formatDay(pause.resumesAt, "long")} from days you'd already paid. Billing resumes then.`
    : "Paused, keeping the days you'd already paid.";
}

function seatsWord(count: number) {
  return `${count} ${count === 1 ? "seat" : "seats"}`;
}

function countWord(count: number, seats: boolean) {
  return seats ? seatsWord(count) : `${count} ${count === 1 ? "box" : "boxes"}`;
}

/** A change asked for at renewal that differs from what is paid now. */
export function heldChange(sub: Subscription): ScheduledChange | undefined {
  const next = sub.atRenewal;
  return next &&
    (next.quantity !== sub.quantity ||
      next.interval !== sub.interval ||
      next.product !== sub.product)
    ? next
    : undefined;
}

type Preview =
  | { for: string; state: "loading" }
  | { for: string; state: "ready"; preview: ChangePreview }
  | { for: string; state: "failed"; problem: string };

/** What a change would charge, asked once the change has stopped moving: the route shares the change's rate limit. */
function useChangePreview(
  subId: string,
  change?: SubscriptionChange,
): Preview | undefined {
  const client = useClient();
  const key = change ? JSON.stringify(change) : "";
  const [seen, setSeen] = useState<Preview>();
  useEffect(() => {
    if (!key) return;
    let current = true;
    const timer = setTimeout(() => {
      void (async () => {
        try {
          const preview = await (
            await client("see what the change costs")
          ).previewSubscriptionChange(
            subId,
            JSON.parse(key) as SubscriptionChange,
          );
          if (current) setSeen({ for: key, state: "ready", preview });
        } catch (error) {
          if (current)
            setSeen({ for: key, state: "failed", problem: problemText(error) });
        }
      })();
    }, 450);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [key, subId, client]);
  if (!key) return undefined;
  return seen?.for === key ? seen : { for: key, state: "loading" };
}

function PreviewWords({
  seen,
  when,
  interval,
  until,
}: {
  seen?: Preview;
  when: ChangeWhen;
  interval: BillingInterval;
  until?: string;
}) {
  if (!seen || seen.state === "failed") return null;
  if (seen.state === "loading")
    return <span className="quiet">Working out the amount…</span>;
  const { now, renewal } = seen.preview;
  if (when === "renewal")
    return (
      <span className="quiet">
        {renewal
          ? `From ${formatDay(renewal.at, "long")}: ${money(renewal.amount, { cents: false })} a ${interval}.`
          : "It ends at the end of this period, so nothing more is charged."}
        {until ? ` ${until}` : ""}
      </span>
    );
  const then = renewal
    ? `${money(renewal.amount, { cents: false })} a ${interval} from ${formatDay(renewal.at, "long")}`
    : undefined;
  return now.amount > 0 ? (
    <span>
      {money(now)} today
      <span className="quiet">
        {" "}
        for what's left of this period{then ? `, then ${then}` : ""}.
      </span>
    </span>
  ) : (
    <span>
      Nothing to pay today
      <span className="quiet">{then ? `, then ${then}` : ""}.</span>
    </span>
  );
}

function previewReady(seen?: Preview): ChangePreview | undefined {
  return seen?.state === "ready" ? seen.preview : undefined;
}

function chargedToday(preview?: ChangePreview) {
  return preview && preview.now.amount > 0
    ? ` ${money(preview.now)} charged today.`
    : "";
}

function AtRenewal({
  sub,
  catalog,
  pays,
  reload,
  show,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  pays: boolean;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const action = useAction();
  const held = heldChange(sub);
  if (!held) return null;
  const product = productOf(catalog, sub.product);
  const name = product?.name ?? sub.product;
  const seats = product?.unit === "seat";
  const ends = formatDay(sub.currentPeriodEnd, "long");
  const kept =
    held.product !== sub.product
      ? name
      : held.quantity !== sub.quantity
        ? countWord(sub.quantity, seats)
        : intervalWord(sub.interval);
  return (
    <Row
      label="At renewal"
      action={
        pays ? (
          <button
            type="button"
            className="button small"
            onClick={() =>
              void action.run(
                async () => {
                  await (
                    await client("keep your plan as it is")
                  ).changeSubscription(sub.id, {
                    product: sub.product,
                    interval: sub.interval,
                    quantity: sub.quantity,
                  });
                  reload();
                  show(`${name} renews as it is now.`);
                },
                (error) =>
                  `${problemText(error)} The change still happens on ${ends}.`,
              )
            }
            disabled={action.busy}
            data-busy={action.busy || undefined}
          >
            Keep {kept}
          </button>
        ) : null
      }
      below={
        action.problem ? (
          <p className="problem" role="alert">
            {action.problem}
          </p>
        ) : null
      }
    >
      <span>
        {[
          held.product !== sub.product
            ? productOf(catalog, held.product)?.name
            : undefined,
          held.quantity !== sub.quantity
            ? countWord(held.quantity, seats)
            : undefined,
          held.interval !== sub.interval
            ? intervalWord(held.interval)
            : undefined,
        ]
          .filter(Boolean)
          .join(", ")}
      </span>
      <span className="quiet">· from {ends}</span>
    </Row>
  );
}

export function ChangeSeats({
  sub,
  name,
  max = 1000,
  min = 1,
  onClose,
  onChanged,
}: {
  sub: Subscription;
  name: string;
  max?: number;
  min?: number;
  onClose: () => void;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const [count, setCount] = useState(Math.max(sub.quantity, min));
  const added = count - sub.quantity;
  const when: ChangeWhen = added > 0 ? "now" : "renewal";
  const change = added === 0 ? undefined : { quantity: count, when };
  const seen = useChangePreview(sub.id, change);
  const ends = formatDay(sub.currentPeriodEnd, "long");
  return (
    <Confirm
      problem={
        action.problem ?? (seen?.state === "failed" ? seen.problem : undefined)
      }
      busy={action.busy}
      unchanged={!previewReady(seen)}
      label={
        added > 0
          ? `Add ${seatsWord(added)}`
          : `Go to ${seatsWord(count)} at renewal`
      }
      onCancel={onClose}
      onConfirm={() =>
        void action.run(
          async () => {
            if (!change) return;
            const preview = previewReady(seen);
            await (
              await client("change your seats")
            ).changeSubscription(sub.id, change);
            onClose();
            onChanged(
              added > 0
                ? `${seatsWord(added)} added.${chargedToday(preview)}`
                : `${name} goes to ${seatsWord(count)} on ${ends}.`,
            );
          },
          (error) => `${problemText(error)} Nothing was charged.`,
        )
      }
    >
      <label className="stepper-line">
        <Stepper value={count} min={min} max={max} onChange={setCount} />
        {change ? (
          <PreviewWords
            seen={seen}
            when={when}
            interval={sub.interval}
            until={
              when === "renewal"
                ? `All ${seatsWord(sub.quantity)} stay until then.`
                : undefined
            }
          />
        ) : (
          <span className="quiet">
            seats. More are charged today for what's left of this period; fewer
            start at renewal.
          </span>
        )}
      </label>
    </Confirm>
  );
}

export function Stepper({
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

export function useCheckout() {
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
  interval,
  access,
}: {
  catalog: BillingCatalog;
  canBuy: boolean;
  interval: BillingInterval;
  access: TeamAccess;
}) {
  const pro = productOf(catalog, "pro");
  const team = productOf(catalog, "team");
  const checkout = useCheckout();
  const [naming, setNaming] = useState(false);
  const proMonth = priceOf(pro, "month");
  const proYear = priceOf(pro, "year");

  return (
    <Section label="Upgrade">
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
        {team && (access === "sold" || access === "preview") ? (
          <PlanOffer
            catalog={catalog}
            plan="team"
            price={priceOf(team, "month")}
            per={`${UNIT_WORD.seat} ${PER.month}`}
            other={access === "preview" ? FREE_DURING_PREVIEW : undefined}
            added={access === "preview" ? inPreview : undefined}
          >
            {access === "preview" ? (
              <a className="button" href={TEAM}>
                Start Team
              </a>
            ) : naming ? (
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
  added: shown = () => true,
  children,
}: {
  catalog: BillingCatalog;
  plan: "pro" | "team";
  price?: Money;
  per: string;
  other?: string;
  added?: (feature: PlanFeature) => boolean;
  children: ReactNode;
}) {
  const { before, added } = featuresAdded(catalog, plan);
  return (
    <PlanCard
      name={catalog.plans.find((each) => each.id === plan)?.name ?? plan}
      price={price}
      per={other ? `${per} · ${other}` : per}
      plus={before ? `Everything in ${before}, plus` : undefined}
      lines={added.filter(shown).map((feature) => ({
        key: feature,
        text: FEATURE_WORDS[feature],
      }))}
    >
      {children}
    </PlanCard>
  );
}

export function NewTeam({
  catalog,
  checkout,
  submit = "Start Team",
  onCancel,
}: {
  catalog: BillingCatalog;
  checkout: ReturnType<typeof useCheckout>;
  submit?: string;
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
        maxLength={64}
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
          {submit}
        </button>
      </div>
    </form>
  );
}

/** Makes an org in the free preview, with no checkout. */
export function NewPreviewTeam({
  onMade,
  onCancel,
}: {
  onMade: (org: Org) => void;
  onCancel: () => void;
}) {
  const client = useClient();
  const action = useAction();
  const [name, setName] = useState("");
  return (
    <form
      className="new-team"
      onSubmit={(event) => {
        event.preventDefault();
        if (!name.trim()) return;
        void action.run(
          async () => {
            const routes = await client("start a Team");
            onMade(await routes.createOrg({ name: name.trim() }));
          },
          (error) => `${problemText(error)} No org was made.`,
        );
      }}
    >
      <input
        className="field"
        placeholder="Org name"
        aria-label="Org name"
        value={name}
        onChange={(event) => setName(event.target.value)}
        autoFocus
        maxLength={64}
      />
      <span className="quiet">{FREE_DURING_PREVIEW}</span>
      {action.problem ? (
        <p className="problem" role="alert">
          {action.problem}
        </p>
      ) : null}
      <div className="actions">
        <button
          type="button"
          className="button small"
          onClick={onCancel}
          disabled={action.busy}
        >
          Cancel
        </button>
        <button
          type="submit"
          className="button small primary"
          disabled={!name.trim() || action.busy}
          data-busy={action.busy || undefined}
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
    .filter(
      (sub) =>
        sub.status === "active" && !sub.cancelAtPeriodEnd && !sub.seatPause,
    )
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

type BoxAsk = { interval: BillingInterval } | { product: BillingProductId };

export function ShedBox({
  sub,
  catalog,
  canBuy,
  pays,
  reload,
  show,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  canBuy: boolean;
  pays: boolean;
  reload: () => void;
  show: Show;
}) {
  const [asked, setAsked] = useState<BoxAsk>();
  const product = productOf(catalog, sub.product);
  const name = product?.name ?? sub.product;
  const refunding = !!sub.refundRequestedAt;
  const open = pays && canBuy && !refunding && !heldChange(sub);
  const sizing = !!asked && "product" in asked;
  return (
    <>
      <Row
        label={name}
        action={
          open ? (
            <button
              type="button"
              className="button small"
              onClick={() =>
                setAsked(sizing ? undefined : { product: sub.product })
              }
              aria-expanded={sizing}
            >
              Change size
            </button>
          ) : null
        }
        below={
          asked ? (
            <ChangeBox
              sub={sub}
              catalog={catalog}
              asked={asked}
              onAsk={setAsked}
              onClose={() => setAsked(undefined)}
              onChanged={(text) => {
                reload();
                show(text);
              }}
            />
          ) : null
        }
      >
        {refunding ? (
          <span>Ends when its refund is paid</span>
        ) : open ? (
          <Segmented
            label={`How ${name} is billed`}
            value={asked && "interval" in asked ? asked.interval : sub.interval}
            onChange={(interval) =>
              setAsked(interval === sub.interval ? undefined : { interval })
            }
            options={(["month", "year"] as const)
              .filter((interval) => priceOf(product, interval))
              .map((interval) => {
                const unit = priceOf(product, interval);
                return {
                  value: interval,
                  name: `${interval === "month" ? "Monthly" : "Yearly"}${unit ? ` · ${money(unit, { cents: false })}` : ""}`,
                };
              })}
          />
        ) : (
          <span>{intervalWord(sub.interval)}</span>
        )}
        {sub.quantity > 1 ? (
          <span className="quiet">· {sub.quantity} boxes</span>
        ) : null}
      </Row>
      <AtRenewal
        sub={sub}
        catalog={catalog}
        pays={pays}
        reload={reload}
        show={show}
      />
    </>
  );
}

function ChangeBox({
  sub,
  catalog,
  asked,
  onAsk,
  onClose,
  onChanged,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  asked: BoxAsk;
  onAsk: (asked: BoxAsk) => void;
  onClose: () => void;
  onChanged: (text: string) => void;
}) {
  const client = useClient();
  const action = useAction();
  const from = productOf(catalog, sub.product);
  const name = from?.name ?? sub.product;
  const ends = formatDay(sub.currentPeriodEnd, "long");
  const to = "product" in asked ? productOf(catalog, asked.product) : from;
  const interval = "interval" in asked ? asked.interval : sub.interval;
  const larger =
    (priceOf(to, "month")?.amount ?? 0) > (priceOf(from, "month")?.amount ?? 0);
  const when: ChangeWhen = larger ? "now" : "renewal";
  const change: SubscriptionChange | undefined =
    "interval" in asked
      ? { interval, when: "renewal" }
      : to && to.id !== sub.product
        ? { product: to.id, when }
        : undefined;
  const seen = useChangePreview(sub.id, change);
  const toName = to?.name ?? sub.product;
  const sizes = catalog.products.filter(
    (product) => product.kind === "shed" && priceOf(product, sub.interval),
  );
  return (
    <Confirm
      problem={
        action.problem ?? (seen?.state === "failed" ? seen.problem : undefined)
      }
      busy={action.busy}
      unchanged={!previewReady(seen)}
      label={
        "interval" in asked
          ? `Go ${intervalWord(interval)} at renewal`
          : when === "now"
            ? `Move to ${toName}`
            : `Move to ${toName} at renewal`
      }
      onCancel={() => {
        action.clear();
        onClose();
      }}
      onConfirm={() =>
        void action.run(
          async () => {
            if (!change) return;
            const preview = previewReady(seen);
            await (
              await client("change your box")
            ).changeSubscription(sub.id, change);
            onClose();
            onChanged(
              "interval" in asked
                ? `${name} goes ${intervalWord(interval)} on ${ends}.`
                : when === "now"
                  ? `${name} is now ${toName}.${chargedToday(preview)}`
                  : `${name} becomes ${toName} on ${ends}.`,
            );
          },
          (error) => `${problemText(error)} Nothing was charged.`,
        )
      }
    >
      <span className="stepper-line">
        {"product" in asked ? (
          <Segmented
            label="Box size"
            value={asked.product}
            onChange={(product) => onAsk({ product })}
            options={sizes.map((size) => ({
              value: size.id,
              name: size.name,
            }))}
          />
        ) : null}
        {change ? (
          <PreviewWords
            seen={seen}
            when={change.when ?? "now"}
            interval={interval}
            until={
              change.when === "renewal" && "product" in asked
                ? `It stays ${name} until then.`
                : undefined
            }
          />
        ) : (
          <span className="quiet">
            A larger box starts today; a smaller one at renewal.
          </span>
        )}
      </span>
    </Confirm>
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

export function Refunds({
  subs,
  readAt,
  catalog,
  reload,
  show,
}: {
  subs: Subscription[];
  readAt: number;
  catalog: BillingCatalog;
  reload: () => void;
  show: Show;
}) {
  const open = subs.filter(
    (sub) =>
      sub.refundRequestedAt ||
      (sub.refundableUntil && Date.parse(sub.refundableUntil) > readAt),
  );
  if (open.length === 0) return null;
  return (
    <Section label="Refund">
      <Rows>
        {open.map((sub) => (
          <RefundRow
            key={sub.id}
            sub={sub}
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
  sub,
  catalog,
  reload,
  show,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const [asking, setAsking] = useState(false);
  const action = useAction();
  const plan = productOf(catalog, sub.product)?.name ?? sub.product;
  const bought = formatDay(sub.startedAt, "long");
  const name = `${plan}, bought ${bought}`;
  const amount = sub.refundAmount ? money(sub.refundAmount) : undefined;
  const again =
    productOf(catalog, sub.product)?.kind === "shed" ? "a Shed box" : plan;

  if (sub.refundRequestedAt || !sub.refundableUntil)
    return (
      <ItemRow
        glyph={<IconUndo size={16} />}
        name={name}
        detail={`Refund${amount ? ` of ${amount}` : ""} asked${sub.refundRequestedAt ? ` on ${formatDay(sub.refundRequestedAt, "long")}` : ""}. It takes a few days to come back, and ${plan} ends then.`}
      />
    );

  const until = formatDay(sub.refundableUntil, "long");
  return (
    <ItemRow
      glyph={<IconUndo size={16} />}
      name={name}
      detail={`${amount ?? "Everything paid on it so far"} comes back if you ask by ${until}. It's given once.`}
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
          label={amount ? `Refund ${amount}` : "Ask for the refund"}
          onCancel={() => setAsking(false)}
          onConfirm={() =>
            void action.run(
              async () => {
                await (
                  await client("ask for a refund")
                ).refundSubscription(sub.id);
                setAsking(false);
                reload();
                show("Refund asked. It takes a few days to come back.");
              },
              (error) =>
                `${problemText(error)} Nothing was refunded. Reload the page to see where ${plan} stands.`,
            )
          }
        >
          <p>
            Ask for a refund of {plan}? Everything paid on it since {bought}
            {amount ? `, ${amount},` : ""} comes back. The payment provider
            takes a few days, and {plan} ends once it has. A refund is given
            once for each plan, so buying {again} again has none.
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
  disputed: "Disputed",
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
        {invoice.partlyRefunded
          ? `${money(invoice.partlyRefunded)} refunded`
          : STATUS_WORDS[invoice.status]}
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
