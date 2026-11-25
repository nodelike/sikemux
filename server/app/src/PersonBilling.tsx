import { formatDay } from "@shared/lib/format";
import type {
  BillingCatalog,
  BillingInterval,
  Org,
  Subscription,
  TeamAccess,
} from "@sikemux/protocol";
import { useState, type MouseEvent } from "react";

import { problemText } from "./api.ts";
import { OrgAvatar } from "./Avatar.tsx";
import { BilledSwitch } from "./BilledSwitch.tsx";
import {
  BillNotLoaded,
  CancelPlanConfirm,
  canPay,
  CardLine,
  cardName,
  ChangePlanDialog,
  FeatureList,
  FREE_DURING_PREVIEW,
  heldChange,
  intervalWord,
  InvoiceTable,
  isRefundable,
  mayChange,
  NOT_OPEN,
  openSubs,
  pausedLine,
  planSubOf,
  RefundConfirm,
  seatsWord,
  TestMode,
  useBill,
  useCheckout,
  usePortal,
  useShow,
  type BillWear,
  type LoadedBill,
  type PlanAsk,
} from "./BillingShared.tsx";
import { useClient } from "./client.ts";
import { iconMark } from "./marks.ts";
import {
  featuresAdded,
  inPreview,
  money,
  priceOf,
  priceText,
  productOf,
} from "./money.ts";
import { orgPath, START } from "./navigation.ts";
import { frozen } from "./orgs.ts";
import { ShedBoxes } from "./Shed.tsx";
import {
  Btn,
  btnClass,
  Ending,
  Page,
  Problem,
  Row,
  Rows,
  Section,
  Txt,
  useAction,
} from "./ui.tsx";
import "./billing.css";

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
  const wear: BillWear = { mark: iconMark("card"), scene: "you" };
  if (load.state !== "loaded")
    return (
      <BillNotLoaded
        load={load}
        reload={reload}
        wear={wear}
        lede="Your own plan."
        failTitle="The plan didn't load"
        failBody="Your hosts and agents keep working."
      />
    );
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
  loaded: LoadedBill;
  wear: BillWear;
  staff: boolean;
  team: TeamAccess;
  orgs: Org[];
  follow: (to: string) => (event: MouseEvent<HTMLAnchorElement>) => void;
  reload: () => void;
}) {
  const { catalog, overview, invoices, readAt } = loaded;
  const { allowance, live, canBuy } = overview;
  const client = useClient();
  const show = useShow();
  const checkout = useCheckout();
  const portal = usePortal({});
  const resumeAction = useAction();
  const keepAction = useAction();
  const [interval, setInterval] = useState<BillingInterval>("month");
  const [planAsk, setPlanAsk] = useState<PlanAsk>();
  const [cancelling, setCancelling] = useState(false);
  const [refundSub, setRefundSub] = useState<Subscription>();

  const open = openSubs(overview);
  const planSub = planSubOf(loaded);
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
  const proUnit = priceOf(pro, interval) ?? priceOf(pro, "month");
  const proMonth = priceOf(pro, "month");
  const proYear = priceOf(pro, "year");
  const monthsFree =
    proMonth && proYear ? 12 - Math.round(proYear.amount / proMonth.amount) : 0;

  const subProduct = planSub ? productOf(catalog, planSub.product) : undefined;
  const subPrice =
    planSub && subProduct ? priceText(subProduct, planSub.interval) : undefined;
  const ends = planSub ? formatDay(planSub.currentPeriodEnd, "long") : "";
  const lapsed = planSub?.status === "lapsed";
  const refunding = Boolean(planSub?.refundRequestedAt);
  const held = planSub ? heldChange(planSub) : undefined;
  const heldPrice =
    held && subProduct ? priceText(subProduct, held.interval) : undefined;

  const yearly = priceOf(subProduct, "year");
  const monthly = priceOf(subProduct, "month");
  const goesYearly =
    planSub?.status === "active" &&
    planSub.interval === "month" &&
    yearly &&
    mayChange(planSub, pays, canBuy);
  const goesMonthly =
    planSub?.status === "active" &&
    planSub.interval === "year" &&
    monthly &&
    mayChange(planSub, pays, canBuy);

  const heroTitle = lapsed
    ? `${subProduct?.name ?? planName} is off`
    : seat
      ? `${planName}, through ${new Intl.ListFormat("en").format(seatIn)}`
      : free
        ? "Free"
        : giftEnds
          ? `${planName}, a gift`
          : refunding
            ? `${planName}, ending ${ends}`
            : planSub?.cancelAtPeriodEnd
              ? `${planName}, until ${ends}`
              : planName;

  const heroDetail = lapsed
    ? `Its last payment failed, so ${subProduct?.name ?? planName}'s features are off until it's paid. Your hosts keep working.`
    : seat
      ? `Your ${seatIn.length > 1 ? "seats there give" : "seat there gives"} you Pro on your own hosts${planSub?.seatPause && subPrice ? `, so your own ${subPrice} plan is paused` : ""}.`
      : free
        ? "Every agent, on every host, for nothing."
        : giftEnds
          ? `Yours free until ${giftEnds}. After that you're on Free, and nothing you have is deleted.`
          : planSub
            ? refunding
              ? `${planSub.refundAmount ? `${money(planSub.refundAmount)} is on its way back` : "A refund is on its way"}${overview.card ? ` to ${cardName(overview.card)}` : ""}. It ends when its refund is paid.`
              : planSub.seatPause
                ? pausedLine(planSub.seatPause)
                : planSub.cancelAtPeriodEnd
                  ? "Then Free. Your hosts and agents keep working."
                  : held
                    ? `${subPrice ? `${subPrice} now. ` : ""}From ${ends}, ${heldPrice ?? intervalWord(held.interval)}.`
                    : `${subPrice ? `${subPrice}. ` : ""}Renews on ${ends}.`
            : undefined;

  const ownedOrgs =
    team === "sold" ? orgs.filter((org) => org.role === "owner") : [];
  const showMoneyColumn =
    pays &&
    (Boolean(planSub) || free || invoices.length > 0 || Boolean(overview.card));
  const canRefundPlan = isRefundable(planSub, readAt);
  const shedRefunds = shedSubs.filter(
    (sub) => sub.refundRequestedAt || isRefundable(sub, readAt),
  );
  const heroProblem =
    checkout.problem ??
    portal.problem ??
    resumeAction.problem ??
    keepAction.problem;

  return (
    <Page
      {...wear}
      tone={lapsed ? "failed" : undefined}
      title="Billing"
      tag={<TestMode live={live} />}
      lede="Your own plan."
    >
      <div className={lapsed ? "bill-hero bad" : "bill-hero"}>
        <h2>{heroTitle}</h2>
        {heroDetail ? <p>{heroDetail}</p> : null}
        {free && !planSub && pro ? (
          <div className="bill-buy">
            <BilledSwitch
              interval={interval}
              onChange={setInterval}
              saving={pro}
            />
            {proUnit ? (
              <span className="bill-price-note">
                <b>
                  {money(proUnit, { cents: false })} a {interval}
                </b>
                {interval === "year" && monthsFree > 0 ? (
                  <small>{monthsFree} months free</small>
                ) : null}
              </span>
            ) : null}
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
              {canBuy ? "Go Pro" : NOT_OPEN}
            </Btn>
          </div>
        ) : lapsed && pays ? (
          <div className="bill-buy">
            <Btn busy={portal.busy} onClick={() => void portal.open()}>
              Change the card
            </Btn>
          </div>
        ) : planSub?.cancelAtPeriodEnd && !refunding && pays ? (
          <div className="bill-buy">
            <Btn
              busy={resumeAction.busy}
              onClick={() =>
                void resumeAction.run(
                  async () => {
                    await (
                      await client("keep your plan")
                    ).resumeSubscription(planSub.id);
                    reload();
                    show(`${planName} renews on ${ends}.`);
                  },
                  (error) => `${problemText(error)} It still ends on ${ends}.`,
                )
              }
            >
              Keep {planName}
            </Btn>
          </div>
        ) : held && planSub && !refunding && pays ? (
          <div className="bill-buy">
            <Btn
              quiet
              busy={keepAction.busy}
              onClick={() =>
                void keepAction.run(
                  async () => {
                    await (
                      await client("keep your plan as it is")
                    ).changeSubscription(planSub.id, {
                      product: planSub.product,
                      interval: planSub.interval,
                      quantity: planSub.quantity,
                    });
                    reload();
                    show(`${planName} renews as it is now.`);
                  },
                  (error) =>
                    `${problemText(error)} The change still happens on ${ends}.`,
                )
              }
            >
              Stay {intervalWord(planSub.interval)}
            </Btn>
          </div>
        ) : goesYearly ? (
          <div className="bill-buy">
            <Btn quiet onClick={() => setPlanAsk({ interval: "year" })}>
              Pay yearly, {money(yearly, { cents: false })}
            </Btn>
          </div>
        ) : goesMonthly ? (
          <div className="bill-buy">
            <Btn quiet onClick={() => setPlanAsk({ interval: "month" })}>
              Pay monthly, {money(monthly, { cents: false })}
            </Btn>
          </div>
        ) : null}
        <Problem>{heroProblem}</Problem>
      </div>

      <div className="bill-cols">
        <Section
          label={
            lapsed ? "What Pro has" : free ? "What Pro adds" : "What you have"
          }
        >
          <FeatureList features={featuresAdded(catalog, "pro").added} />
        </Section>
        <div className="bill-side">
          {showMoneyColumn ? (
            <>
              <Section label="Card">
                <CardLine card={overview.card} declined={lapsed} query={{}} />
              </Section>
              <Section
                label="Invoices"
                count={invoices.length || undefined}
                foot={
                  invoices.length === 0
                    ? `What you pay for a plan in its first ${catalog.refundDays} days comes back in full if you ask within them, once for each plan.`
                    : undefined
                }
              >
                <InvoiceTable compact invoices={invoices} catalog={catalog} />
              </Section>
            </>
          ) : null}
          {ownedOrgs.length > 0 ? (
            <Section
              label={
                ownedOrgs.length === 1 ? "Org you pay for" : "Orgs you pay for"
              }
            >
              <Rows>
                {ownedOrgs.map((org) => {
                  const to = orgPath(org.id, "billing");
                  return (
                    <Row
                      key={org.id}
                      lead={<OrgAvatar org={org} size={28} />}
                      name={org.name}
                      detail={
                        org.preview
                          ? "Team preview"
                          : frozen(org)
                            ? "Team ended"
                            : `Team, ${seatsWord(org.seats.limit)}`
                      }
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
            </Section>
          ) : null}
        </div>
      </div>

      {seatOrgs === 0 ? (
        <TeamOffer catalog={catalog} team={team} orgs={orgs} follow={follow} />
      ) : null}

      <ShedBoxes
        catalog={catalog}
        bought={shedSubs}
        buying={staff}
        canBuy={canBuy}
        pays={pays}
        reload={reload}
        show={show}
      />

      {shedRefunds.length > 0 ? (
        <Section label="Shed refunds">
          <Rows>
            {shedRefunds.map((sub) => {
              const name = productOf(catalog, sub.product)?.name ?? sub.product;
              const bought = formatDay(sub.startedAt, "long");
              const amount = sub.refundAmount
                ? money(sub.refundAmount)
                : undefined;
              if (sub.refundRequestedAt)
                return (
                  <Row
                    key={sub.id}
                    name={`${name}, bought ${bought}`}
                    detail={`Refund${amount ? ` of ${amount}` : ""} asked on ${formatDay(sub.refundRequestedAt, "long")}. It takes a few days to come back, and ${name} ends then.`}
                  />
                );
              return (
                <Row
                  key={sub.id}
                  name={`${name}, bought ${bought}`}
                  detail={`${amount ?? "Everything paid on it so far"} comes back${sub.refundableUntil ? ` if you ask by ${formatDay(sub.refundableUntil, "long")}` : ""}. It's given once.`}
                  end={
                    <Txt onClick={() => setRefundSub(sub)}>
                      Ask for a refund
                    </Txt>
                  }
                />
              );
            })}
          </Rows>
        </Section>
      ) : null}

      {planSub && pays && !refunding && !planSub.cancelAtPeriodEnd ? (
        <Ending
          title={`Cancel ${planName}`}
          actions={
            <>
              {canRefundPlan ? (
                <Txt onClick={() => setRefundSub(planSub)}>
                  Ask for a refund
                </Txt>
              ) : null}
              <Btn bad onClick={() => setCancelling(true)}>
                Cancel {planName}
              </Btn>
            </>
          }
        >
          {planSub.seatPause
            ? `${planName} ends now, and the days you'd already paid are refunded.`
            : `You keep it until ${ends}.${canRefundPlan ? ` What you pay in its first ${catalog.refundDays} days can be refunded.` : ""}`}
        </Ending>
      ) : planSub && pays && !refunding && canRefundPlan ? (
        <Ending
          title="Something wrong with a charge?"
          actions={
            <Txt onClick={() => setRefundSub(planSub)}>Ask for a refund</Txt>
          }
        >
          What you pay for a plan in its first {catalog.refundDays} days comes
          back in full if you ask within them, once for each plan.
        </Ending>
      ) : null}

      {planSub && planAsk ? (
        <ChangePlanDialog
          sub={planSub}
          catalog={catalog}
          asked={planAsk}
          onAsk={setPlanAsk}
          onClose={() => setPlanAsk(undefined)}
          onChanged={(text) => {
            reload();
            show(text);
          }}
        />
      ) : null}

      {planSub && cancelling ? (
        <CancelPlanConfirm
          sub={planSub}
          name={planName}
          onClose={() => setCancelling(false)}
          reload={reload}
          show={show}
        />
      ) : null}

      {refundSub ? (
        <RefundConfirm
          sub={refundSub}
          catalog={catalog}
          onClose={() => setRefundSub(undefined)}
          reload={reload}
          show={show}
        />
      ) : null}
    </Page>
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
    <Section
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
      <FeatureList
        features={featuresAdded(catalog, "team").added.filter(
          (feature) => !preview || inPreview(feature),
        )}
      />
    </Section>
  );
}
