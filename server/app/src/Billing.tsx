import type {
  BillingCatalog,
  BillingOverview,
  ChangeWhen,
  Invoice,
  Org,
  OrgMember,
  PlanFeature,
  Subscription,
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
import { useState, type ReactNode } from "react";

import { problemText } from "./api.ts";
import {
  canPay,
  chargedToday,
  countWord,
  heldChange,
  intervalWord,
  mayChange,
  NOT_OPEN,
  openSubs,
  pausedLine,
  planSubOf,
  previewReady,
  PreviewWords,
  seatsWord,
  Stepper,
  TestMode,
  useBill,
  useChangePreview,
  useCheckout,
  useClient,
  usePortal,
  useShow,
  type BillLoad as Load,
  type BillWear as Wear,
  type LoadedBill as Loaded,
  type Show,
} from "./BillingShared.tsx";
import { useOrgMark } from "./logos.ts";
import {
  FEATURE_WORDS,
  featuresAdded,
  inPreview,
  money,
  priceOf,
  priceText,
  productOf,
  times,
} from "./money.ts";
import { frozen, useLoad } from "./orgs.ts";
import { orgScene } from "./scenes.ts";
import { Seats } from "./Seats.tsx";
import {
  Btn,
  Section,
  Chip,
  Confirm,
  Empty,
  Page,
  Problem,
  Row,
  Rows,
  Tile,
  Txt,
  useAction,
} from "./ui.tsx";

export { BillingPage } from "./PersonBilling.tsx";
export {
  FREE_DURING_PREVIEW,
  heldChange,
  NewTeam,
  NOT_OPEN,
  ShedBox,
  Stepper,
  useBillingShown,
  useCheckout,
  useClient,
  type Show,
} from "./BillingShared.tsx";

type Ask = "seats" | "cancel";

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
        <Section label="Plan" busy />
        <Section label="Card" busy />
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
  const client = useClient();
  const members = useLoad(
    async () =>
      (await (await client("see your org")).listOrgMembers(org.id)).members,
    String(version),
  );
  const mark = useOrgMark(org);
  const wear: Wear = {
    mark,
    scene: orgScene(org.id),
  };
  if (load.state !== "loaded")
    return <NotLoaded load={load} reload={reload} wear={wear} />;
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
  loaded: Loaded;
  members: { data?: OrgMember[]; problem?: string };
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
  const planSub = planSubOf(loaded);
  const pays = canPay(overview);
  const product = productOf(catalog, "team");
  const unit = priceOf(product, "month");
  const each = unit ? money(unit, { cents: false }) : undefined;
  const ends = planSub ? formatDay(planSub.currentPeriodEnd, "long") : "";
  const lapsed = planSub?.status === "lapsed";
  const preview = org.preview && !planSub && !frozen(org);
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
      {frozen(org) ? null : members.data ? (
        <Seats org={org} members={members.data} />
      ) : (
        <Section label="Seats" busy={!members.problem}>
          <Problem>{members.problem}</Problem>
        </Section>
      )}
      {planSub ? (
        <Section label="Plan">
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
        </Section>
      ) : preview ? (
        <Section label="What happens next">
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
        </Section>
      ) : sells ? (
        <Section label="Seats">
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
        </Section>
      ) : null}
      {paused ? null : (
        <Section label="What Team has">
          <Features
            features={featuresAdded(catalog, "team").added.filter(
              (feature) => !preview || inPreview(feature),
            )}
          />
        </Section>
      )}
      {paused && org.seats.held > 0 ? (
        <Section label="Seats">
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
        </Section>
      ) : null}
      {preview && invoices.length === 0 ? (
        <Section label="Invoices">
          <Empty>None while it's free.</Empty>
        </Section>
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
    </Page>
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
      <Section
        label="Invoices"
        foot={`What you pay for a plan in its first ${catalog.refundDays} days comes back in full if you ask within them, once for each plan.`}
      >
        <Empty>No card and no invoices yet.</Empty>
      </Section>
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
    <Section label={renewing.length > 1 ? "Next charges" : "Next charge"}>
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
    </Section>
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
    <Section label="Card">
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
  return (
    <Section label="Invoices" count={invoices.length || undefined}>
      {invoices.length === 0 ? (
        <Empty>None yet.</Empty>
      ) : (
        <Rows>
          {invoices.map((invoice) => (
            <InvoiceRow key={invoice.id} invoice={invoice} catalog={catalog} />
          ))}
        </Rows>
      )}
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
