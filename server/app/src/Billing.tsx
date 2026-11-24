import { useAuth } from "@clerk/react";
import type {
  BillingCatalog,
  BillingInterval,
  BillingOverview,
  BillingProductId,
  ChangePreview,
  ChangeWhen,
  Invoice,
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
  IconCheck,
  IconClock,
  IconCreditCard,
  IconDownload,
  IconLaptop,
  IconMail,
  IconRefresh,
  IconScrollText,
  IconSwap,
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

import { ApiProblem } from "@shared/account/api";
import { api, problemText, signedInToken } from "./api.ts";
import { OrgAvatar } from "./Avatar.tsx";
import { orgSvg } from "./avatarSvg.ts";
import { BilledSwitch } from "./BilledSwitch.tsx";
import { useCheer } from "./Field.tsx";
import { brandMark } from "./marks.ts";
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
} from "./money.ts";
import { orgPath, START } from "./navigation.ts";
import { frozen } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import { ShedBoxes } from "./Shed.tsx";
import {
  Btn,
  btnClass,
  Card,
  Chip,
  Column,
  Confirm,
  Empty,
  Field,
  Page,
  Problem,
  Row,
  Rows,
  Seg,
  Tile,
  Txt,
  useAction,
  useToast,
} from "./ui.tsx";

export const FREE_DURING_PREVIEW = "Free during preview";
export const NOT_OPEN = "Buying is not open yet";

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
export function useBillingShown(
  ready: boolean,
  version: number,
): boolean | undefined {
  const client = useClient();
  const [shown, setShown] = useState<boolean>();
  useEffect(() => {
    if (!ready) return;
    let current = true;
    void client("see your plan")
      .then((routes) => routes.getBilling())
      .then(
        () => current && setShown(true),
        (error: unknown) =>
          current &&
          setShown((last) =>
            error instanceof ApiProblem && error.status === 404
              ? false
              : (last ?? false),
          ),
      );
    return () => {
      current = false;
    };
  }, [ready, client, version]);
  return shown;
}

/** What one payer, the person or an org, is billed. Read again whenever `version` moves. */
function useBill(version: number, orgId?: string) {
  const client = useClient();
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let current = true;
    const query = orgId ? { org: orgId } : {};
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
  }, [orgId, version, tick, client]);
  const reload = useCallback(() => setTick((n) => n + 1), []);
  return { load, reload };
}

export type Show = (text: string, undo?: () => Promise<void>) => void;

function useShow(): Show {
  const say = useToast();
  const cheer = useCheer();
  return useCallback(
    (text, undo) => {
      say(text, undo);
      cheer();
    },
    [say, cheer],
  );
}

type Ask = "seats" | "cancel" | { interval: BillingInterval };

type Wear = Pick<Parameters<typeof Page>[0], "mark" | "scene">;

function NotLoaded({
  load,
  reload,
  wear,
}: {
  load: Exclude<Load, { state: "loaded" }>;
  reload: () => void;
  wear: Wear;
}) {
  if (load.state === "loading")
    return (
      <Page {...wear} title="Plan and billing">
        <Card label="Plan" w={7} busy />
        <Column w={5} side>
          <Card label="Card" busy />
        </Column>
      </Page>
    );
  return (
    <Page
      {...wear}
      tone="failed"
      title="The plan didn't load"
      lede="Nothing about it has changed."
      actions={<Btn onClick={reload}>Try again</Btn>}
    >
      <div className="full-err">
        <Problem>{load.problem}</Problem>
      </div>
    </Page>
  );
}

const TestMode = ({ live }: { live: boolean }) =>
  live ? null : <Chip tone="warn">TEST MODE</Chip>;

const openSubs = (overview: BillingOverview) =>
  overview.subscriptions.filter((sub) => sub.status !== "ended");

function usePlanSub({ catalog, overview }: Loaded) {
  return openSubs(overview).find(
    (sub) => productOf(catalog, sub.product)?.kind === "plan",
  );
}

const canPay = (overview: BillingOverview) =>
  overview.payer !== "another_owner" && overview.payer !== "departed";

/** The person's own plan and bill. */
export function BillingPage({
  version,
  staff,
  team,
  orgs,
  follow,
}: {
  version: number;
  staff: boolean;
  team: TeamAccess;
  orgs: Org[];
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const { load, reload } = useBill(version);
  const wear: Wear = { mark: brandMark, scene: "plan" };
  if (load.state !== "loaded")
    return <NotLoaded load={load} reload={reload} wear={wear} />;
  return (
    <PersonBill
      loaded={load}
      wear={wear}
      staff={staff}
      team={team}
      orgs={orgs}
      follow={follow}
      reload={reload}
    />
  );
}

function PersonBill({
  loaded,
  wear,
  staff,
  team,
  orgs,
  follow,
  reload,
}: {
  loaded: Loaded;
  wear: Wear;
  staff: boolean;
  team: TeamAccess;
  orgs: Org[];
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  reload: () => void;
}) {
  const { catalog, overview, invoices, readAt } = loaded;
  const { allowance, live, canBuy } = overview;
  const show = useShow();
  const checkout = useCheckout();
  const portal = usePortal({});
  const [interval, setInterval] = useState<BillingInterval>("month");
  const [ask, setAsk] = useState<Ask>();
  const open = openSubs(overview);
  const planSub = usePlanSub(loaded);
  const shedSubs = open.filter(
    (sub) => productOf(catalog, sub.product)?.kind === "shed",
  );
  const pays = canPay(overview);
  const planName =
    catalog.plans.find((plan) => plan.id === allowance.plan)?.name ??
    allowance.plan;
  const free = allowance.plan === "free";
  const seatIn = orgs
    .filter((each) => allowance.seatOrgs?.includes(each.id))
    .map((each) => each.name);
  const seat = seatIn.length > 0;
  const seatOrgs = allowance.seatOrgs?.length ?? 0;
  const giftEnds =
    !planSub && allowance.giftEnds
      ? formatDay(allowance.giftEnds, "long")
      : undefined;
  const pro = productOf(catalog, "pro");
  const proPrice =
    (pro ? priceText(pro, interval) : undefined) ??
    (pro ? priceText(pro, "month") : undefined);
  const subProduct = planSub ? productOf(catalog, planSub.product) : undefined;
  const subPrice =
    planSub && subProduct ? priceText(subProduct, planSub.interval) : undefined;
  const ends = planSub ? formatDay(planSub.currentPeriodEnd, "long") : "";
  const lapsed = planSub?.status === "lapsed";
  const yearly = priceOf(subProduct, "year");
  const goesYearly =
    planSub?.status === "active" &&
    planSub.interval === "month" &&
    yearly &&
    mayChange(planSub, pays, canBuy);

  const lede = lapsed
    ? `Its last payment failed, so ${subProduct?.name ?? planName}'s features are off until it's paid. Your hosts keep working.`
    : seat
      ? `Your ${seatIn.length > 1 ? "seats there give" : "seat there gives"} you Pro on your own hosts${planSub?.seatPause && subPrice ? `, so your own ${subPrice} plan is paused` : ""}.`
      : free
        ? "Every agent, on every host, for nothing."
        : giftEnds
          ? `Yours free until ${giftEnds}. After that you're on Free, and nothing you have is deleted.`
          : planSub
            ? `${subPrice ? `${subPrice}. ` : ""}${planSub.refundRequestedAt ? "It ends when its refund is paid." : planSub.seatPause ? pausedLine(planSub.seatPause) : planSub.cancelAtPeriodEnd ? `Ends on ${ends}.` : `Renews on ${ends}.`}`
            : undefined;

  return (
    <Page
      {...wear}
      tone={lapsed ? "failed" : undefined}
      title={
        lapsed
          ? "The card was declined"
          : seat
            ? `${planName}, through ${seatIn.join(" and ")}`
            : planName
      }
      tag={<TestMode live={live} />}
      lede={lede}
      actions={
        lapsed && pays ? (
          <Btn busy={portal.busy} onClick={() => void portal.open()}>
            Change the card
          </Btn>
        ) : free && !planSub && pro ? (
          <>
            <Btn
              disabled={!canBuy}
              busy={checkout.busy}
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
              {canBuy
                ? `Go Pro${proPrice ? ` for ${proPrice}` : ""}`
                : NOT_OPEN}
            </Btn>
            <BilledSwitch
              interval={interval}
              onChange={setInterval}
              saving={pro}
            />
          </>
        ) : goesYearly ? (
          <Btn quiet onClick={() => setAsk({ interval: "year" })}>
            Pay yearly, {money(yearly, { cents: false })}
          </Btn>
        ) : null
      }
    >
      {checkout.problem || portal.problem ? (
        <div className="full-err">
          <Problem>{checkout.problem ?? portal.problem}</Problem>
        </div>
      ) : null}
      <Column w={7}>
        {planSub ? (
          <Card label="Plan">
            <Rows>
              <PlanRows
                sub={planSub}
                catalog={catalog}
                canBuy={canBuy}
                pays={pays}
                ask={ask}
                onAsk={setAsk}
                reload={reload}
                show={show}
              />
            </Rows>
          </Card>
        ) : null}
        <Card
          label={
            lapsed ? "What Pro has" : free ? "What Pro adds" : "What you have"
          }
        >
          <Features features={featuresAdded(catalog, "pro").added} />
        </Card>
        <OrgBills team={team} orgs={orgs} follow={follow} />
        {seatOrgs > 0 ? null : (
          <TeamOffer
            catalog={catalog}
            team={team}
            orgs={orgs}
            follow={follow}
          />
        )}
        <ShedBoxes
          catalog={catalog}
          bought={shedSubs}
          buying={staff}
          canBuy={canBuy}
          pays={pays}
          reload={reload}
          show={show}
        />
      </Column>
      <Column w={5} side>
        <Money
          overview={overview}
          invoices={invoices}
          readAt={readAt}
          catalog={catalog}
          pays={pays}
          query={{}}
          reload={reload}
          show={show}
        />
      </Column>
    </Page>
  );
}

function OrgBills({
  team,
  orgs,
  follow,
}: {
  team: TeamAccess;
  orgs: Org[];
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const owned = orgs.filter((org) => org.role === "owner");
  if (team !== "sold" || owned.length === 0) return null;
  return (
    <Card label={owned.length === 1 ? "Your org's bill" : "Your orgs' bills"}>
      <Rows>
        {owned.map((org) => {
          const to = orgPath(org.id, "billing");
          return (
            <Row
              key={org.id}
              lead={<OrgAvatar id={org.id} size={28} />}
              name={org.name}
              detail={
                org.preview
                  ? "Team preview"
                  : frozen(org)
                    ? "Team ended"
                    : `Team, ${seatsWord(org.seats.limit)}`
              }
              hot={{
                picture: { svg: orgSvg(org.id) },
                scene: orgScene(org.id),
              }}
              end={
                <a
                  className={btnClass({ small: true, quiet: true })}
                  href={to}
                  onClick={follow(to)}
                >
                  Open
                </a>
              }
            />
          );
        })}
      </Rows>
    </Card>
  );
}

function TeamOffer({
  catalog,
  team,
  orgs,
  follow,
}: {
  catalog: BillingCatalog;
  team: TeamAccess;
  orgs: Org[];
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
}) {
  const product = productOf(catalog, "team");
  if (
    !product ||
    (team !== "sold" && team !== "preview") ||
    orgs.some((org) => org.role === "owner")
  )
    return null;
  const preview = team === "preview";
  return (
    <Card
      label="Team"
      aside={
        <span className="soft">
          {preview ? FREE_DURING_PREVIEW : priceText(product, "month")}
        </span>
      }
      foot={
        <a
          className={btnClass({ small: true })}
          href={START}
          onClick={follow(START)}
        >
          Start an org
        </a>
      }
    >
      <Features
        features={featuresAdded(catalog, "team").added.filter(
          (feature) => !preview || inPreview(feature),
        )}
      />
    </Card>
  );
}

const FEATURE_ICONS: Partial<Record<PlanFeature, ReactNode>> = {
  sync: <IconLaptop size={14} />,
  move_work: <IconSwap size={14} />,
  history_search: <IconScrollText size={14} />,
  own_triggers: <IconClock size={14} />,
};

function Features({ features }: { features: PlanFeature[] }) {
  return (
    <Rows>
      {features.map((feature) => {
        const [name, detail] = FEATURE_WORDS[feature].split(": ");
        return (
          <Row
            key={feature}
            lead={
              <Tile>{FEATURE_ICONS[feature] ?? <IconCheck size={14} />}</Tile>
            }
            name={name}
            detail={
              detail
                ? `${detail.charAt(0).toUpperCase()}${detail.slice(1)}.`
                : undefined
            }
          />
        );
      })}
    </Rows>
  );
}

/** The org's own plan and bill, for its owners. */
export function OrgBillingPage({
  org,
  version,
  team,
}: {
  org: Org;
  version: number;
  team: TeamAccess;
}) {
  const { load, reload } = useBill(version, org.id);
  const wear: Wear = {
    mark: { svg: orgSvg(org.id) },
    scene: orgScene(org.id),
  };
  if (load.state !== "loaded")
    return <NotLoaded load={load} reload={reload} wear={wear} />;
  return (
    <OrgBill loaded={load} wear={wear} org={org} team={team} reload={reload} />
  );
}

function OrgBill({
  loaded,
  wear,
  org,
  team,
  reload,
}: {
  loaded: Loaded;
  wear: Wear;
  org: Org;
  team: TeamAccess;
  reload: () => void;
}) {
  const { catalog, overview, invoices, readAt } = loaded;
  const { live, canBuy } = overview;
  const show = useShow();
  const query = { org: org.id };
  const checkout = useCheckout();
  const portal = usePortal(query);
  const [ask, setAsk] = useState<Ask>();
  const planSub = usePlanSub(loaded);
  const pays = canPay(overview);
  const product = productOf(catalog, "team");
  const unit = priceOf(product, "month");
  const each = unit ? money(unit, { cents: false }) : undefined;
  const ends = planSub ? formatDay(planSub.currentPeriodEnd, "long") : "";
  const lapsed = planSub?.status === "lapsed";
  const preview = org.preview && !planSub;
  const ended = overview.subscriptions
    .filter((sub) => sub.status === "ended" && sub.product === "team")
    .sort((a, b) => b.currentPeriodEnd.localeCompare(a.currentPeriodEnd))[0];
  const lastSeats = ended?.quantity ?? 1;
  const least = Math.max(1, org.seats.held);
  const [seats, setSeats] = useState(Math.max(least, lastSeats));
  const sells = team === "sold" && !planSub && !preview;
  const paused = team !== "sold" && !planSub && !preview;
  const total = unit ? money(times(unit, seats), { cents: false }) : undefined;
  const nextTotal =
    planSub && unit
      ? money(times(unit, (planSub.atRenewal ?? planSub).quantity), {
          cents: false,
        })
      : undefined;

  const title = preview
    ? "Team preview"
    : lapsed
      ? "The card was declined"
      : planSub
        ? "Team"
        : paused
          ? "Team paused"
          : ended
            ? "Team ended"
            : "No Team plan";
  const lede = preview
    ? `Free for up to ${org.seats.limit} people while we finish Team.`
    : lapsed
      ? `Its last payment failed. Invites and org features are frozen until it's paid; everyone keeps their own hosts.${pays ? "" : " Only whoever pays for it can change the card."}`
      : planSub
        ? overview.payer === "another_owner"
          ? `${seatsWord(planSub.quantity)}. Another owner pays for it, so only they can change it.`
          : overview.payer === "departed"
            ? `Whoever paid for it no longer owns the org, so it ends on ${ends} and can be bought again then.`
            : `${seatsWord(planSub.quantity)}${each ? ` at ${each} each` : ""}. ${planSub.refundRequestedAt ? "It ends when its refund is paid." : planSub.cancelAtPeriodEnd ? `It ends on ${ends}.` : `The next charge is${nextTotal ? ` ${nextTotal}` : ""} on ${ends}.`}`
        : paused
          ? `${org.name} is paused. Team isn't open to your account now. Nothing is deleted.`
          : ended
            ? `Its Team plan ended on ${formatDay(ended.currentPeriodEnd, "long")}. Nothing is deleted.`
            : "Nobody can be invited until it has one. Nothing is deleted.";

  return (
    <Page
      {...wear}
      tone={lapsed ? "failed" : !planSub && !preview ? "waiting" : undefined}
      title={title}
      tag={<TestMode live={live} />}
      lede={lede}
      actions={
        lapsed ? (
          pays ? (
            <Btn busy={portal.busy} onClick={() => void portal.open()}>
              Change the card
            </Btn>
          ) : null
        ) : planSub ? (
          mayChange(planSub, pays, canBuy) ? (
            <Btn onClick={() => setAsk("seats")}>Change seats</Btn>
          ) : null
        ) : sells ? (
          <Btn
            disabled={!canBuy}
            busy={checkout.busy}
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
            {canBuy
              ? `${ended ? "Renew" : "Start Team"}${total ? ` for ${total} a month` : ""}`
              : NOT_OPEN}
          </Btn>
        ) : null
      }
    >
      {checkout.problem || portal.problem ? (
        <div className="full-err">
          <Problem>{checkout.problem ?? portal.problem}</Problem>
        </div>
      ) : null}
      <Column w={7}>
        {frozen(org) ? null : <Seats org={org} />}
        {planSub ? (
          <Card label="Plan">
            <Rows>
              <PlanRows
                sub={planSub}
                catalog={catalog}
                canBuy={canBuy}
                pays={pays}
                held={org.seats.held}
                ask={ask}
                onAsk={setAsk}
                reload={reload}
                show={show}
              />
            </Rows>
          </Card>
        ) : preview ? (
          <Card label="What happens next">
            <Rows>
              <Row
                lead={
                  <Tile>
                    <IconMail size={14} />
                  </Tile>
                }
                name="30 days' notice"
                detail="We write to owners before it costs anything."
              />
              <Row
                lead={
                  <Tile>
                    <IconCheck size={14} />
                  </Tile>
                }
                name="Nothing is lost"
                detail="People, domains and the log carry over."
              />
              {each ? (
                <Row
                  lead={
                    <Tile>
                      <IconCreditCard size={14} />
                    </Tile>
                  }
                  name={`${each} a seat each month, after`}
                  detail="You choose whether to keep it."
                />
              ) : null}
            </Rows>
          </Card>
        ) : sells ? (
          <Card label="Seats">
            <Rows>
              <Row
                lead={
                  <Tile>
                    <IconUsers size={14} />
                  </Tile>
                }
                name={seatsWord(seats)}
                detail={
                  org.seats.held > 0
                    ? `${org.seats.held} held by people and links out`
                    : undefined
                }
                end={
                  <Stepper
                    value={seats}
                    min={least}
                    max={product?.maxQuantity ?? 1000}
                    onChange={setSeats}
                  />
                }
              />
            </Rows>
          </Card>
        ) : null}
        {paused ? null : (
          <Card label="What Team has">
            <Features
              features={featuresAdded(catalog, "team").added.filter(
                (feature) => !preview || inPreview(feature),
              )}
            />
          </Card>
        )}
        {paused && org.seats.held > 0 ? (
          <Card label="Seats">
            <Rows>
              <Row
                lead={
                  <Tile>
                    <IconUsers size={14} />
                  </Tile>
                }
                name={`${seatsWord(org.seats.held)} held`}
                detail="People stay in it, and keep their own hosts."
              />
            </Rows>
          </Card>
        ) : null}
      </Column>
      <Column w={5} side>
        {preview && invoices.length === 0 ? (
          <Card label="Invoices">
            <Empty>None while it's free.</Empty>
          </Card>
        ) : (
          <Money
            overview={overview}
            invoices={invoices}
            readAt={readAt}
            catalog={catalog}
            pays={pays}
            query={query}
            reload={reload}
            show={show}
          />
        )}
      </Column>
    </Page>
  );
}

const DRAWN_SEATS = 60;

function Seats({ org }: { org: Org }) {
  const { held, limit } = org.seats;
  const left = Math.max(0, limit - held);
  return (
    <Card label="Seats">
      <div className="seatw">
        <div>
          <div className="big">
            {held}
            <small>of {limit}</small>
          </div>
          <p className="soft">
            {left === 0 ? "None free" : `${left} free`}. People and links out
            hold a seat.
          </p>
        </div>
        {limit <= DRAWN_SEATS ? (
          <div className="seats" aria-hidden="true">
            {Array.from({ length: limit }, (_, index) => (
              <span key={index} className={index < held ? "s h" : "s f"} />
            ))}
          </div>
        ) : null}
      </div>
    </Card>
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

function intervalWord(interval: BillingInterval) {
  return interval === "month" ? "monthly" : "yearly";
}

/** Whether the payer can change it now: not while it is refunding, paused, or holding a change. */
function mayChange(sub: Subscription, pays: boolean, canBuy: boolean) {
  return (
    pays &&
    canBuy &&
    !sub.refundRequestedAt &&
    !sub.seatPause &&
    !heldChange(sub)
  );
}

function PlanRows({
  sub,
  catalog,
  canBuy,
  pays,
  held: taken,
  ask,
  onAsk,
  reload,
  show,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  canBuy: boolean;
  pays: boolean;
  /** Seats people and links out hold already; fewer can't be paid for. */
  held?: number;
  ask?: Ask;
  onAsk: (ask?: Ask) => void;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const action = useAction();
  const product = productOf(catalog, sub.product);
  const name = product?.name ?? sub.product;
  const seats = product?.unit === "seat";
  const ends = formatDay(sub.currentPeriodEnd, "long");
  const refunding = !!sub.refundRequestedAt;
  const price = product ? priceText(product, sub.interval) : undefined;
  const pause = sub.seatPause;
  const open = mayChange(sub, pays, canBuy);
  const bothWays =
    sub.status === "active" &&
    !!priceOf(product, "month") &&
    !!priceOf(product, "year");
  const billing = typeof ask === "object" ? ask.interval : undefined;
  const resume = async () => {
    await (await client("keep your plan")).resumeSubscription(sub.id);
    reload();
  };
  const close = () => {
    onAsk(undefined);
    action.clear();
  };

  return (
    <>
      <Row
        lead={
          <Tile>
            {seats ? <IconUsers size={14} /> : <IconUser size={14} />}
          </Tile>
        }
        name={seats ? seatsWord(sub.quantity) : name}
        detail={
          [
            price,
            taken !== undefined
              ? `${taken} held, ${Math.max(0, sub.quantity - taken)} free`
              : undefined,
          ]
            .filter(Boolean)
            .join(" · ") || undefined
        }
        end={
          seats && open ? (
            <Btn
              small
              quiet
              aria-expanded={ask === "seats"}
              onClick={() => (ask === "seats" ? close() : onAsk("seats"))}
            >
              Change seats
            </Btn>
          ) : seats && pays && !canBuy ? (
            <span>{NOT_OPEN}</span>
          ) : null
        }
      >
        {seats && open && ask === "seats" ? (
          <ChangeSeats
            sub={sub}
            name={name}
            max={product?.maxQuantity}
            min={Math.max(1, taken ?? 1)}
            onClose={close}
            onChanged={(text) => {
              reload();
              show(text);
            }}
          />
        ) : null}
      </Row>
      {sub.product === "team" ? (
        <Row
          lead={
            <Tile>
              <IconLaptop size={14} />
            </Tile>
          }
          name="Pro on your own hosts"
          detail="While you hold a seat."
        />
      ) : null}
      {bothWays && open ? (
        <Row
          lead={
            <Tile>
              <IconCreditCard size={14} />
            </Tile>
          }
          name="Billed"
          detail={intervalWord(sub.interval).replace(/^./, (first) =>
            first.toUpperCase(),
          )}
          end={
            <IntervalSeg
              label={`How ${name} is billed`}
              sub={sub}
              catalog={catalog}
              value={billing ?? sub.interval}
              onChange={(interval) =>
                onAsk(interval === sub.interval ? undefined : { interval })
              }
            />
          }
        >
          {billing && billing !== sub.interval ? (
            <ChangePlan
              sub={sub}
              catalog={catalog}
              asked={{ interval: billing }}
              onAsk={() => undefined}
              onClose={close}
              onChanged={(text) => {
                reload();
                show(text);
              }}
            />
          ) : null}
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
        <Row
          lead={
            <Tile>
              <IconUndo size={14} />
            </Tile>
          }
          name="Ends when its refund is paid"
          detail="It can't be changed until then."
        />
      ) : sub.cancelAtPeriodEnd ? (
        <Row
          lead={
            <Tile>
              <IconClock size={14} />
            </Tile>
          }
          name={`Ends on ${ends}`}
          detail={
            sub.product === "team"
              ? "Invites and org features freeze then."
              : undefined
          }
          end={
            pays ? (
              <Btn
                small
                busy={action.busy}
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
              >
                Keep {name}
              </Btn>
            ) : null
          }
        >
          {action.problem && ask !== "cancel" ? (
            <Problem>{action.problem}</Problem>
          ) : null}
        </Row>
      ) : (
        <Row
          lead={
            <Tile>
              <IconRefresh size={14} />
            </Tile>
          }
          name={pause ? (pause.resumesAt ? "Resumes" : "Paused") : "Renews"}
          detail={
            pause
              ? pausedLine(pause)
              : `On ${ends}, ${intervalWord(sub.interval)}`
          }
          end={
            <Txt
              bad
              aria-expanded={ask === "cancel"}
              onClick={() => (ask === "cancel" ? close() : onAsk("cancel"))}
            >
              Cancel {name}
            </Txt>
          }
        >
          {ask === "cancel" ? (
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
                      show(`${name} cancelled. It runs until ${ends}.`, resume);
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
          ) : null}
        </Row>
      )}
    </>
  );
}

function IntervalSeg({
  label,
  sub,
  catalog,
  value,
  onChange,
}: {
  label: string;
  sub: Subscription;
  catalog: BillingCatalog;
  value: BillingInterval;
  onChange: (interval: BillingInterval) => void;
}) {
  const product = productOf(catalog, sub.product);
  return (
    <Seg
      label={label}
      value={value}
      onChange={onChange}
      options={(["month", "year"] as const).flatMap((interval) => {
        const unit = priceOf(product, interval);
        return unit
          ? [
              {
                value: interval,
                name: `${interval === "month" ? "Monthly" : "Yearly"} · ${money(unit, { cents: false })}`,
              },
            ]
          : [];
      })}
    />
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
    return <span className="soft">Working out the amount…</span>;
  const { now, renewal } = seen.preview;
  if (when === "renewal")
    return (
      <span className="soft">
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
      <span className="soft">
        {" "}
        for what's left of this period{then ? `, then ${then}` : ""}.
      </span>
    </span>
  ) : (
    <span>
      Nothing to pay today
      <span className="soft">{then ? `, then ${then}` : ""}.</span>
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
  const becomes = [
    held.product !== sub.product
      ? productOf(catalog, held.product)?.name
      : undefined,
    held.quantity !== sub.quantity
      ? countWord(held.quantity, seats)
      : undefined,
    held.interval !== sub.interval ? intervalWord(held.interval) : undefined,
  ]
    .filter(Boolean)
    .join(", ");
  return (
    <Row
      lead={
        <Tile>
          <IconClock size={14} />
        </Tile>
      }
      name={becomes.replace(/^./, (first) => first.toUpperCase())}
      detail={`From ${ends}`}
      end={
        pays ? (
          <Btn
            small
            quiet
            busy={action.busy}
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
          >
            Keep {kept}
          </Btn>
        ) : null
      }
    >
      {action.problem ? <Problem>{action.problem}</Problem> : null}
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
      <label className="step-line">
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
          <span className="soft">
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

function NameForm({
  submit,
  action,
  onCancel,
  onSubmit,
  children,
}: {
  submit: string;
  action: { busy: boolean; problem?: string };
  onCancel: () => void;
  onSubmit: (name: string) => void;
  children?: ReactNode;
}) {
  const [name, setName] = useState("");
  return (
    <form
      className="box"
      onSubmit={(event) => {
        event.preventDefault();
        if (name.trim()) onSubmit(name.trim());
      }}
    >
      <Field
        placeholder="Org name"
        aria-label="Org name"
        value={name}
        onChange={(event) => setName(event.target.value)}
        autoFocus
        maxLength={64}
      />
      {children}
      <Problem>{action.problem}</Problem>
      <div className="do">
        <Btn type="submit" small busy={action.busy} disabled={!name.trim()}>
          {submit}
        </Btn>
        <Btn small quiet onClick={onCancel} disabled={action.busy}>
          Cancel
        </Btn>
      </div>
    </form>
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
  const [seats, setSeats] = useState(3);
  const team = productOf(catalog, "team");
  const unit = priceOf(team, "month");
  return (
    <NameForm
      submit={submit}
      action={checkout}
      onCancel={onCancel}
      onSubmit={(name) =>
        void checkout.go(async (routes) => {
          const org = await routes.createOrg({ name });
          return routes.startCheckout({
            product: "team",
            interval: "month",
            quantity: seats,
            orgId: org.id,
          });
        })
      }
    >
      <span className="step-line">
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
      </span>
    </NameForm>
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
  return (
    <NameForm
      submit="Start Team"
      action={action}
      onCancel={onCancel}
      onSubmit={(name) =>
        void action.run(
          async () => {
            const routes = await client("start a Team");
            onMade(await routes.createOrg({ name }));
          },
          (error) => `${problemText(error)} No org was made.`,
        )
      }
    >
      <span className="soft">{FREE_DURING_PREVIEW}</span>
    </NameForm>
  );
}

/** Everything about the money itself: what is charged next, the card, refunds and invoices. */
function Money({
  overview,
  invoices,
  readAt,
  catalog,
  pays,
  query,
  reload,
  show,
}: {
  overview: BillingOverview;
  invoices: Invoice[];
  readAt: number;
  catalog: BillingCatalog;
  pays: boolean;
  query: { org?: string };
  reload: () => void;
  show: Show;
}) {
  const open = openSubs(overview);
  if (open.length === 0 && invoices.length === 0)
    return (
      <Card
        label="Invoices"
        foot={`What you pay for a plan in its first ${catalog.refundDays} days comes back in full if you ask within them, once for each plan.`}
      >
        <Empty>No card and no invoices yet.</Empty>
      </Card>
    );
  return (
    <>
      <NextCharge subs={open} catalog={catalog} card={overview.card} />
      {pays ? (
        <PaymentMethod
          card={overview.card}
          declined={open.some((sub) => sub.status === "lapsed")}
          query={query}
        />
      ) : null}
      <Refunds
        subs={open}
        readAt={readAt}
        catalog={catalog}
        reload={reload}
        show={show}
      />
      <Invoices invoices={invoices} catalog={catalog} />
    </>
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
    <Card label={renewing.length > 1 ? "Next charges" : "Next charge"}>
      <Rows>
        {renewing.map(({ sub, next, product, unit }) => (
          <Row
            key={sub.id}
            name={
              product?.unit === "seat"
                ? `${product.name}, ${next.quantity} seats × ${money(unit, { cents: false })}`
                : product?.unit === "box"
                  ? `${product.name}, ${countWord(next.quantity, false)}, ${intervalWord(next.interval)}`
                  : `${product?.name ?? next.product}, ${intervalWord(next.interval)}`
            }
            detail={formatDay(sub.currentPeriodEnd, "long")}
            end={
              <span className="amount">
                {money(times(unit, next.quantity))}
              </span>
            }
          />
        ))}
        {renewing.length > 1 ? (
          <Row
            name="Total"
            detail={card ? cardName(card) : undefined}
            end={
              <span className="amount">
                {money({ amount: total, currency })}
              </span>
            }
          />
        ) : null}
      </Rows>
    </Card>
  );
}

type PlanAsk = { interval: BillingInterval } | { product: BillingProductId };

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
  const [asked, setAsked] = useState<PlanAsk>();
  const product = productOf(catalog, sub.product);
  const name = product?.name ?? sub.product;
  const refunding = !!sub.refundRequestedAt;
  const open = mayChange(sub, pays, canBuy);
  const sizing = !!asked && "product" in asked;
  return (
    <>
      <Row
        wrap
        name={name}
        detail={
          [
            refunding
              ? "Ends when its refund is paid"
              : open
                ? undefined
                : intervalWord(sub.interval),
            sub.quantity > 1 ? countWord(sub.quantity, false) : undefined,
          ]
            .filter(Boolean)
            .join(" · ") || undefined
        }
        end={
          open ? (
            <>
              <IntervalSeg
                label={`How ${name} is billed`}
                sub={sub}
                catalog={catalog}
                value={
                  asked && "interval" in asked ? asked.interval : sub.interval
                }
                onChange={(interval) =>
                  setAsked(interval === sub.interval ? undefined : { interval })
                }
              />
              <Btn
                small
                quiet
                aria-expanded={sizing}
                onClick={() =>
                  setAsked(sizing ? undefined : { product: sub.product })
                }
              >
                Change size
              </Btn>
            </>
          ) : null
        }
      >
        {asked ? (
          <ChangePlan
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

function ChangePlan({
  sub,
  catalog,
  asked,
  onAsk,
  onClose,
  onChanged,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  asked: PlanAsk;
  onAsk: (asked: PlanAsk) => void;
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
              await client("change your plan")
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
      <span className="step-line">
        {"product" in asked ? (
          <Seg
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
          <span className="soft">
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
    <Card label="Refund">
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
    </Card>
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
  const lead = (
    <Tile>
      <IconUndo size={14} />
    </Tile>
  );

  if (sub.refundRequestedAt || !sub.refundableUntil)
    return (
      <Row
        lead={lead}
        name={name}
        detail={`Refund${amount ? ` of ${amount}` : ""} asked${sub.refundRequestedAt ? ` on ${formatDay(sub.refundRequestedAt, "long")}` : ""}. It takes a few days to come back, and ${plan} ends then.`}
      />
    );

  const until = formatDay(sub.refundableUntil, "long");
  return (
    <Row
      lead={lead}
      name={name}
      detail={`${amount ?? "Everything paid on it so far"} comes back if you ask by ${until}. It's given once.`}
      end={
        <Txt
          onClick={() => {
            action.clear();
            setAsking(!asking);
          }}
          aria-expanded={asking}
        >
          Ask for a refund
        </Txt>
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
    </Row>
  );
}

function cardName(card: NonNullable<BillingOverview["card"]>): string {
  const brand = card.brand.charAt(0).toUpperCase() + card.brand.slice(1);
  return `${brand} ending ${card.last4}`;
}

function PaymentMethod({
  card,
  declined,
  query,
}: {
  card?: BillingOverview["card"];
  declined: boolean;
  query: { org?: string };
}) {
  const portal = usePortal(query);
  return (
    <Card label="Card">
      <Rows>
        <Row
          lead={
            <Tile>
              <IconCreditCard size={14} />
            </Tile>
          }
          name={card ? cardName(card) : "No card on file"}
          detail={
            card
              ? `Expires ${String(card.expMonth).padStart(2, "0")}/${String(card.expYear).slice(-2)}`
              : "The payment provider holds the card; we never see it."
          }
          end={
            <>
              {declined ? <Chip tone="bad">Declined</Chip> : null}
              <Btn
                small
                quiet
                busy={portal.busy}
                onClick={() => void portal.open()}
              >
                {card ? "Change" : "Add a card"}
              </Btn>
            </>
          }
        >
          {portal.problem ? <Problem>{portal.problem}</Problem> : null}
        </Row>
      </Rows>
    </Card>
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
  return (
    <Card label="Invoices" count={invoices.length || undefined}>
      {invoices.length === 0 ? (
        <Empty>None yet.</Empty>
      ) : (
        <Rows>
          {invoices.map((invoice) => (
            <InvoiceRow key={invoice.id} invoice={invoice} catalog={catalog} />
          ))}
        </Rows>
      )}
    </Card>
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
  const status = invoice.partlyRefunded
    ? `${money(invoice.partlyRefunded)} refunded`
    : STATUS_WORDS[invoice.status];
  return (
    <Row
      name={invoiceWhat(invoice, catalog)}
      detail={formatDay(invoice.at, "short")}
      end={
        <>
          {invoice.status === "failed" || invoice.status === "disputed" ? (
            <Chip tone="bad">{status}</Chip>
          ) : (
            <span>{status}</span>
          )}
          <span className="amount">{money(invoice.total)}</span>
          <Btn
            small
            quiet
            className="icon-only"
            busy={action.busy}
            onClick={() => void receipt()}
            aria-label="Download the receipt"
            title="Download the receipt"
          >
            <IconDownload size={14} />
          </Btn>
        </>
      }
    >
      {action.problem ? <Problem>{action.problem}</Problem> : null}
    </Row>
  );
}
