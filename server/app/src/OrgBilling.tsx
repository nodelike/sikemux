import { formatDay } from "@shared/lib/format";
import {
  IconCircleAlert,
  IconClock,
  IconCreditCard,
  IconHourglass,
  IconMail,
} from "@shared/ui/Icons";
import type { ChangeWhen, Org, OrgMember, TeamAccess } from "@sikemux/protocol";
import { useEffect, useState } from "react";

import { problemText } from "./api.ts";
import {
  BillNotLoaded,
  CancelPlanConfirm,
  canPay,
  cardExpiry,
  cardName,
  chargedToday,
  heldChange,
  InvoiceTable,
  isRefundable,
  mayChange,
  NOT_OPEN,
  planSubOf,
  previewReady,
  PreviewWords,
  RefundConfirm,
  seatsWord,
  Stepper,
  TestMode,
  useBill,
  useChangePreview,
  useCheckout,
  useClient,
  usePortal,
  useShow,
  type BillWear,
  type LoadedBill,
} from "./BillingShared.tsx";
import { useClient } from "./client.ts";
import { iconMark } from "./marks.ts";
import { money, priceOf, productOf, times } from "./money.ts";
import { frozen, useLoad } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import { MemberAvatar } from "./Seats.tsx";
import {
  Btn,
  Confirm,
  Ending,
  Figure,
  Figures,
  Notice,
  Page,
  Problem,
  Row,
  Rows,
  Section,
  Txt,
  useAction,
} from "./ui.tsx";
import "./billing.css";

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
  const client = useClient();
  const members = useLoad(
    async () =>
      (await (await client("see your org")).listOrgMembers(org.id)).members,
    String(version),
  );
  const wear: BillWear = {
    mark: iconMark("card"),
    scene: orgScene(org.id),
  };
  if (load.state !== "loaded")
    return (
      <BillNotLoaded
        load={load}
        reload={reload}
        wear={wear}
        lede={`Team for ${org.name}.`}
        failTitle="Billing didn't load"
        failBody="We couldn't reach Sikemux. Nothing was changed."
      />
    );
  return (
    <OrgBill
      loaded={load}
      members={members}
      wear={wear}
      org={org}
      team={team}
      reload={reload}
    />
  );
}

function OrgBill({
  loaded,
  members,
  wear,
  org,
  team,
  reload,
}: {
  loaded: LoadedBill;
  members: { data?: OrgMember[]; problem?: string };
  wear: BillWear;
  org: Org;
  team: TeamAccess;
  reload: () => void;
}) {
  const { catalog, overview, invoices, readAt } = loaded;
  const { live, canBuy } = overview;
  const client = useClient();
  const show = useShow();
  const query = { org: org.id };
  const checkout = useCheckout();
  const portal = usePortal(query);
  const resumeAction = useAction();
  const keepAction = useAction();
  const seatAction = useAction();

  const planSub = planSubOf(loaded);
  const pays = canPay(overview);
  const product = productOf(catalog, "team");
  const unit = priceOf(product, "month");
  const each = unit ? money(unit, { cents: false }) : undefined;
  const ends = planSub ? formatDay(planSub.currentPeriodEnd, "long") : "";
  const endsShort = planSub ? formatDay(planSub.currentPeriodEnd, "short") : "";
  const lapsed = planSub?.status === "lapsed";
  const refunding = Boolean(planSub?.refundRequestedAt);
  const ending = Boolean(planSub?.cancelAtPeriodEnd || refunding);
  const held = planSub ? heldChange(planSub) : undefined;
  const preview = org.preview && !planSub && !frozen(org);

  const ended = overview.subscriptions
    .filter((sub) => sub.status === "ended" && sub.product === "team")
    .sort((a, b) => b.currentPeriodEnd.localeCompare(a.currentPeriodEnd))[0];
  const least = Math.max(1, org.seats.held);
  const [buySeats, setBuySeats] = useState(
    Math.max(least, ended?.quantity ?? 1),
  );

  const canEditSeats = Boolean(
    planSub && planSub.status === "active" && mayChange(planSub, pays, canBuy),
  );
  const currentSeats = planSub?.quantity ?? least;
  const [pickedSeats, setPickedSeats] = useState<{
    base: number;
    count: number;
  }>({ base: currentSeats, count: currentSeats });
  const desiredSeats =
    pickedSeats.base === currentSeats
      ? Math.max(least, pickedSeats.count)
      : Math.max(least, currentSeats);
  const setDesiredSeats = (count: number) =>
    setPickedSeats({ base: currentSeats, count });

  const [confirmSeats, setConfirmSeats] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [askingRefund, setAskingRefund] = useState(false);

  const seatDelta = planSub ? desiredSeats - planSub.quantity : 0;
  const seatWhen: ChangeWhen = seatDelta > 0 ? "now" : "renewal";
  const seatChange =
    planSub && canEditSeats && seatDelta !== 0
      ? { quantity: desiredSeats, when: seatWhen }
      : undefined;
  const seatPreview = useChangePreview(planSub?.id ?? "", seatChange);

  const sells = team === "sold" && !planSub && !preview;
  const paused = team !== "sold" && !planSub && !preview;
  const buyTotal = unit
    ? money(times(unit, buySeats), { cents: false })
    : undefined;
  const nextTotal =
    planSub && unit
      ? money(times(unit, (planSub.atRenewal ?? planSub).quantity), {
          cents: false,
        })
      : undefined;
  const failedInvoice = lapsed
    ? invoices.find((invoice) => invoice.status === "failed")
    : undefined;
  const canRefund = isRefundable(planSub, readAt);

  const notice = (() => {
    if (!planSub) return undefined;
    if (lapsed)
      return (
        <Notice
          tone="bad"
          icon={<IconCircleAlert size={18} />}
          title={
            failedInvoice
              ? `The card was declined on ${formatDay(failedInvoice.at, "long")}`
              : "The card was declined"
          }
          actions={
            pays ? (
              <Btn busy={portal.busy} onClick={() => void portal.open()}>
                Change card
              </Btn>
            ) : null
          }
        >
          Invites and org features are frozen until it&apos;s paid; everyone
          keeps their own hosts.
          {pays ? "" : " Only whoever pays for it can change the card."}
        </Notice>
      );
    if (refunding)
      return (
        <Notice icon={<IconClock size={18} />} title="A refund is on its way">
          {planSub.refundAmount
            ? `${money(planSub.refundAmount)} goes back`
            : "Your refund goes back"}
          {overview.card ? ` to ${cardName(overview.card)}` : ""}. Team ends
          once it settles.
        </Notice>
      );
    if (overview.payer === "departed")
      return (
        <Notice
          tone="bad"
          icon={<IconCircleAlert size={18} />}
          title="The person who paid left"
        >
          Whoever paid for it no longer owns the org, so it ends on {ends} and
          can be bought again then.
        </Notice>
      );
    if (overview.payer === "another_owner")
      return (
        <Notice
          icon={<IconCreditCard size={18} />}
          title="Another owner pays for Team"
        >
          Only they can change the card or cancel. You can still see the bill.
        </Notice>
      );
    if (planSub.cancelAtPeriodEnd)
      return (
        <Notice
          tone="cold"
          icon={<IconHourglass size={18} />}
          title={`Team ends on ${ends}`}
          actions={
            pays ? (
              <Btn
                busy={resumeAction.busy}
                onClick={() =>
                  void resumeAction.run(
                    async () => {
                      await (
                        await client("keep your plan")
                      ).resumeSubscription(planSub.id);
                      reload();
                      show(`Team renews on ${ends}.`);
                    },
                    (error) =>
                      `${problemText(error)} It still ends on ${ends}.`,
                  )
                }
              >
                Keep Team
              </Btn>
            ) : null
          }
        >
          Everyone keeps it until then. Keep it and nothing changes.
        </Notice>
      );
    if (held)
      return (
        <Notice
          icon={<IconClock size={18} />}
          title={`${seatsWord(held.quantity)} from ${ends}`}
          actions={
            pays ? (
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
                      show("Team renews as it is now.");
                    },
                    (error) =>
                      `${problemText(error)} The change still happens on ${ends}.`,
                  )
                }
              >
                Keep {seatsWord(planSub.quantity)}
              </Btn>
            ) : null
          }
        >
          All {seatsWord(planSub.quantity)} stay until then.
        </Notice>
      );
    return undefined;
  })();

  const topProblem =
    checkout.problem ??
    portal.problem ??
    resumeAction.problem ??
    keepAction.problem;

  return (
    <Page
      {...wear}
      tone={lapsed ? "failed" : !planSub && !preview ? "waiting" : undefined}
      title="Billing"
      tag={<TestMode live={live} />}
      lede={`Team for ${org.name}.`}
      notice={notice}
    >
      <Problem>{topProblem}</Problem>

      {preview ? (
        <>
          <div className="bill-hero">
            <h2>Free while we finish Team</h2>
            <p>
              Up to {org.seats.limit} people. You&apos;ll have 30 days&apos;
              notice before it costs anything
              {each ? `, then ${each} a seat each month` : ""}, and nothing is
              charged without your yes.
            </p>
          </div>
          <OrgSeats org={org} members={members} totalSeats={org.seats.limit} />
          <Section label="What happens next">
            <Rows>
              <Row
                name="30 days' notice"
                detail="We write to owners before it costs anything."
              />
              <Row
                name="Nothing is lost"
                detail="People, domains and the log carry over."
              />
              {each ? (
                <Row
                  name={`${each} a seat each month, after`}
                  detail="You choose whether to keep it."
                />
              ) : null}
            </Rows>
          </Section>
          <Section label="Invoices" count={invoices.length || undefined}>
            <InvoiceTable
              invoices={invoices}
              catalog={catalog}
              empty="None while it's free."
            />
          </Section>
        </>
      ) : sells ? (
        <>
          <div className="bill-hero">
            <h2>
              {ended
                ? `Team ended on ${formatDay(ended.currentPeriodEnd, "long")}`
                : "No Team plan"}
            </h2>
            <p>
              {ended
                ? `${org.name} is frozen. Its people, groups and rules are kept, and come back the moment you renew.`
                : "Seats give your people Pro on their own hosts, and the org its rules, groups and audit log."}
            </p>
            <div className="bill-buy">
              <Stepper
                large
                value={buySeats}
                min={least}
                max={product?.maxQuantity ?? 1000}
                onChange={setBuySeats}
                label="Seats"
              />
              <span className="bill-price-note">
                <b>{buyTotal ? `${buyTotal} a month` : seatsWord(buySeats)}</b>
                <small>
                  {each
                    ? `${seatsWord(buySeats)} at ${each}`
                    : seatsWord(buySeats)}
                  {org.seats.held > 0 ? ` · ${org.seats.held} held` : ""}
                </small>
              </span>
              <Btn
                disabled={!canBuy}
                busy={checkout.busy}
                onClick={() =>
                  void checkout.go((routes) =>
                    routes.startCheckout({
                      product: "team",
                      interval: "month",
                      quantity: buySeats,
                      orgId: org.id,
                    }),
                  )
                }
              >
                {canBuy ? (ended ? "Renew Team" : "Start Team") : NOT_OPEN}
              </Btn>
            </div>
          </div>
          <Section label="Invoices" count={invoices.length || undefined}>
            <InvoiceTable invoices={invoices} catalog={catalog} />
          </Section>
        </>
      ) : paused ? (
        <>
          <div className="bill-hero">
            <h2>Team paused</h2>
            <p>
              {org.name} is paused. Team isn&apos;t open to your account now.
              Nothing is deleted.
            </p>
          </div>
          {org.seats.held > 0 ? (
            <Section label="Seats">
              <Rows>
                <Row
                  name={`${seatsWord(org.seats.held)} held`}
                  detail="People stay in it, and keep their own hosts."
                />
              </Rows>
            </Section>
          ) : null}
          {invoices.length > 0 ? (
            <Section label="Invoices" count={invoices.length}>
              <InvoiceTable invoices={invoices} catalog={catalog} />
            </Section>
          ) : null}
        </>
      ) : planSub ? (
        <>
          <Figures>
            <Figure
              label={ending ? "Ends" : "Next bill"}
              detail={
                ending
                  ? refunding
                    ? "When the refund settles"
                    : "No more bills"
                  : lapsed && failedInvoice
                    ? `Includes ${money(failedInvoice.total)} unpaid from ${formatDay(failedInvoice.at, "short")}`
                    : lapsed
                      ? "Past due"
                      : `On ${ends}`
              }
            >
              {ending
                ? endsShort
                : lapsed && failedInvoice
                  ? money(failedInvoice.total, { cents: false })
                  : (nextTotal ?? "—")}
            </Figure>
            <Figure
              label="Seats"
              detail={each ? `${each} a seat, a month` : undefined}
            >
              {canEditSeats ? (
                <Stepper
                  large
                  value={desiredSeats}
                  min={least}
                  max={product?.maxQuantity ?? 1000}
                  onChange={setDesiredSeats}
                  label="Seats"
                />
              ) : (
                planSub.quantity
              )}
            </Figure>
            <Figure
              label="Paid with"
              detail={
                !pays ? undefined : overview.card ? (
                  <span className="bill-card-meta">
                    {lapsed ? (
                      <span className="bill-bad">Declined</span>
                    ) : (
                      `${overview.card.brand.charAt(0).toUpperCase()}${overview.card.brand.slice(1)}, ends ${cardExpiry(overview.card)}`
                    )}
                    <Txt onClick={() => void portal.open()}>Change</Txt>
                  </span>
                ) : (
                  <Txt onClick={() => void portal.open()}>Add card</Txt>
                )
              }
            >
              <span className="bill-card-fig">
                <IconCreditCard size={22} />
                {!pays
                  ? overview.payer === "another_owner"
                    ? "Another owner"
                    : "Payer left"
                  : overview.card
                    ? overview.card.last4
                    : "No card"}
              </span>
            </Figure>
          </Figures>

          <OrgSeats
            org={org}
            members={members}
            totalSeats={canEditSeats ? desiredSeats : planSub.quantity}
          />

          {seatChange ? (
            <div className="seat-save">
              <span>
                <PreviewWords
                  seen={seatPreview}
                  when={seatWhen}
                  interval={planSub.interval}
                  until={
                    seatWhen === "renewal"
                      ? `All ${seatsWord(planSub.quantity)} stay until then.`
                      : undefined
                  }
                />
                {seatPreview?.state === "failed" ? (
                  <Problem>{seatPreview.problem}</Problem>
                ) : null}
              </span>
              <Btn quiet onClick={() => setDesiredSeats(planSub.quantity)}>
                Keep {planSub.quantity}
              </Btn>
              <Btn
                disabled={!previewReady(seatPreview)}
                onClick={() => {
                  seatAction.clear();
                  setConfirmSeats(true);
                }}
              >
                {seatDelta > 0
                  ? `Add ${seatsWord(seatDelta)}`
                  : `Drop ${seatsWord(-seatDelta)}`}
              </Btn>
            </div>
          ) : null}

          <Section label="Invoices" count={invoices.length || undefined}>
            <InvoiceTable invoices={invoices} catalog={catalog} />
          </Section>

          {pays && canRefund ? (
            <Ending
              title="Something wrong with a charge?"
              actions={
                <Txt onClick={() => setAskingRefund(true)}>
                  Ask for a refund
                </Txt>
              }
            >
              What you pay for Team in its first {catalog.refundDays} days comes
              back in full if you ask within them, once for each plan.
            </Ending>
          ) : null}

          {pays && !ending ? (
            <Ending
              title="Cancel Team"
              actions={
                <Btn bad onClick={() => setCancelling(true)}>
                  Cancel Team
                </Btn>
              }
            >
              Everyone keeps it until {ends}, the end of the month you paid for.
            </Ending>
          ) : null}

          {confirmSeats && seatChange ? (
            <Confirm
              title={
                seatDelta > 0
                  ? `Add ${seatsWord(seatDelta)}?`
                  : `Drop ${seatsWord(-seatDelta)}?`
              }
              problem={
                seatAction.problem ??
                (seatPreview?.state === "failed"
                  ? seatPreview.problem
                  : undefined)
              }
              busy={seatAction.busy}
              unchanged={!previewReady(seatPreview)}
              label={
                seatDelta > 0
                  ? `Add ${seatsWord(seatDelta)}`
                  : `Go to ${seatsWord(desiredSeats)} at renewal`
              }
              onCancel={() => {
                seatAction.clear();
                setConfirmSeats(false);
              }}
              onConfirm={() =>
                void seatAction.run(
                  async () => {
                    const previewData = previewReady(seatPreview);
                    await (
                      await client("change your seats")
                    ).changeSubscription(planSub.id, seatChange);
                    setConfirmSeats(false);
                    reload();
                    show(
                      seatDelta > 0
                        ? `${seatsWord(seatDelta)} added.${chargedToday(previewData)}`
                        : `Team goes to ${seatsWord(desiredSeats)} on ${ends}.`,
                    );
                  },
                  (error) => `${problemText(error)} Nothing was charged.`,
                )
              }
            >
              <p>
                <PreviewWords
                  seen={seatPreview}
                  when={seatWhen}
                  interval={planSub.interval}
                  until={
                    seatWhen === "renewal"
                      ? `All ${seatsWord(planSub.quantity)} stay until then.`
                      : undefined
                  }
                />
              </p>
            </Confirm>
          ) : null}

          {cancelling ? (
            <CancelPlanConfirm
              sub={planSub}
              name="Team"
              onClose={() => setCancelling(false)}
              reload={reload}
              show={show}
            />
          ) : null}

          {askingRefund ? (
            <RefundConfirm
              sub={planSub}
              catalog={catalog}
              onClose={() => setAskingRefund(false)}
              reload={reload}
              show={show}
            />
          ) : null}
        </>
      ) : null}
    </Page>
  );
}

const DOT_STRIDE = 36;
const DOT_GAP = 6;

/** Real seat counts, with avatar/invite/free dots drawn only when they fit cleanly without a short orphan row. */
function OrgSeats({
  org,
  members,
  totalSeats,
}: {
  org: Org;
  members: { data?: OrgMember[]; problem?: string };
  totalSeats: number;
}) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const [width, setWidth] = useState(0);

  useEffect(() => {
    if (!box) return;
    const measure = () => setWidth(box.clientWidth);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    return () => observer.disconnect();
  }, [box]);

  if (!members.data)
    return members.problem ? (
      <Problem>{members.problem}</Problem>
    ) : (
      <Section label="Seats" busy />
    );

  const peopleCount = members.data.length;
  const invitedCount = Math.max(0, org.seats.held - peopleCount);
  const freeCount = Math.max(0, totalSeats - peopleCount - invitedCount);
  const totalDots = peopleCount + invitedCount + freeCount;
  const perRow = Math.floor((width + DOT_GAP) / DOT_STRIDE);
  const rows = perRow > 0 ? Math.ceil(totalDots / perRow) : 0;
  const remainder = perRow > 0 ? totalDots % perRow : 0;
  const balanced =
    rows === 1 ||
    (rows === 2 && (remainder === 0 || remainder >= Math.ceil(perRow * 0.6)));
  const drawDots = totalDots > 0 && perRow > 0 && balanced;

  return (
    <div className="seat-strip" ref={setBox}>
      {drawDots ? (
        <div className="seat-dots" aria-hidden="true">
          {members.data.map((member) => (
            <MemberAvatar key={member.id} member={member} size={30} />
          ))}
          {Array.from({ length: invitedCount }, (_, at) => (
            <span key={`inv${at}`} className="seat-inv">
              <IconMail size={13} />
            </span>
          ))}
          {Array.from({ length: freeCount }, (_, at) => (
            <span key={`free${at}`} className="seat-free" />
          ))}
        </div>
      ) : null}
      <p className="seat-counts">
        <span>
          <b>{peopleCount}</b> {peopleCount === 1 ? "person" : "people"}
        </span>
        <span>
          <b>{invitedCount}</b> invited
        </span>
        <span>
          <b>{freeCount}</b> free
        </span>
      </p>
    </div>
  );
}
