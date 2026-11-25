import { formatDay } from "@shared/lib/format";
import {
  IconCheck,
  IconClock,
  IconRefresh,
  IconSearch,
  IconSwap,
} from "@shared/ui/Icons";
import type {
  BillingCatalog,
  BillingInterval,
  BillingOverview,
  BillingProductId,
  ChangePreview,
  ChangeWhen,
  Invoice,
  PlanFeature,
  ScheduledChange,
  SeatPause,
  Subscription,
  SubscriptionChange,
} from "@sikemux/protocol";
import { useCallback, useEffect, useState, type ReactNode } from "react";

import { problemText } from "./api.ts";
import { useClient } from "./client.ts";
import { useCheer } from "./Field.tsx";
import {
  FEATURE_WORDS,
  money,
  PER,
  priceOf,
  productOf,
  times,
} from "./money.ts";
import {
  Btn,
  Chip,
  Confirm,
  Empty,
  Field,
  Nothing,
  Page,
  Problem,
  Row,
  Section,
  Seg,
  Table,
  Td,
  Tr,
  Txt,
  useAction,
  useToast,
  Who,
} from "./ui.tsx";
import "./billing.css";

export const FREE_DURING_PREVIEW = "Free during preview";
export const NOT_OPEN = "Buying is not open yet";

export type LoadedBill = {
  catalog: BillingCatalog;
  overview: BillingOverview;
  invoices: Invoice[];
  /** When it was read, which decides whether a refund is still open. */
  readAt: number;
};

export type BillLoad =
  | { state: "loading" }
  | { state: "failed"; problem: string }
  | ({ state: "loaded" } & LoadedBill);

export type Show = (text: string, undo?: () => Promise<void>) => void;

export type BillWear = Pick<Parameters<typeof Page>[0], "mark" | "scene">;

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
export function useBill(version: number, orgId?: string) {
  const client = useClient();
  const [load, setLoad] = useState<BillLoad>({ state: "loading" });
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

export function useShow(): Show {
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

export function usePortal(query: { org?: string }) {
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

export function TestMode({ live }: { live: boolean }) {
  return live ? null : <Chip tone="warn">TEST MODE</Chip>;
}

/** What has not ended. The API ends a plan whose payer left, though the provider may not have said so yet. */
export function openSubs(overview: BillingOverview) {
  return overview.subscriptions
    .filter((sub) => sub.status !== "ended")
    .map((sub) =>
      overview.payer === "departed" ? { ...sub, cancelAtPeriodEnd: true } : sub,
    );
}

export function planSubOf({ catalog, overview }: LoadedBill) {
  return openSubs(overview).find(
    (sub) => productOf(catalog, sub.product)?.kind === "plan",
  );
}

export function canPay(overview: BillingOverview) {
  return overview.payer !== "another_owner" && overview.payer !== "departed";
}

/** Whether the payer can change it now: not while it is refunding, cancelled, paused, or holding a change. */
export function mayChange(sub: Subscription, pays: boolean, canBuy: boolean) {
  return (
    pays &&
    canBuy &&
    !sub.refundRequestedAt &&
    !sub.cancelAtPeriodEnd &&
    !sub.seatPause &&
    !heldChange(sub)
  );
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

export function isRefundable(sub: Subscription | undefined, readAt: number) {
  return Boolean(
    sub &&
    !sub.refundRequestedAt &&
    sub.refundableUntil &&
    Date.parse(sub.refundableUntil) > readAt,
  );
}

export function intervalWord(interval: BillingInterval) {
  return interval === "month" ? "monthly" : "yearly";
}

export function seatsWord(count: number) {
  return `${count} ${count === 1 ? "seat" : "seats"}`;
}

export function countWord(count: number, seats: boolean) {
  return seats ? seatsWord(count) : `${count} ${count === 1 ? "box" : "boxes"}`;
}

export function cardName(card: NonNullable<BillingOverview["card"]>): string {
  const brand = card.brand.charAt(0).toUpperCase() + card.brand.slice(1);
  return `${brand} ending ${card.last4}`;
}

export function cardExpiry(card: NonNullable<BillingOverview["card"]>): string {
  return `${String(card.expMonth).padStart(2, "0")}/${String(card.expYear).slice(-2)}`;
}

/** What a Pro paused for a seat runs on, in one line. */
export function pausedLine(pause: SeatPause) {
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

export function BillNotLoaded({
  load,
  reload,
  wear,
  lede,
  failTitle,
  failBody,
}: {
  load: Exclude<BillLoad, { state: "loaded" }>;
  reload: () => void;
  wear: BillWear;
  lede: string;
  failTitle: string;
  failBody: string;
}) {
  if (load.state === "loading")
    return (
      <Page {...wear} title="Billing" lede={lede}>
        <Section label="Plan" busy />
        <Section label="Invoices" busy />
      </Page>
    );
  return (
    <Page {...wear} tone="failed" title="Billing" lede={lede}>
      <Nothing
        title={failTitle}
        actions={
          <Btn quiet onClick={reload}>
            <IconRefresh size={14} />
            Try again
          </Btn>
        }
      >
        {failBody}
      </Nothing>
      <Problem>{load.problem}</Problem>
    </Page>
  );
}

export function Stepper({
  value,
  min,
  max,
  onChange,
  large,
  label = "How many",
}: {
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
  large?: boolean;
  label?: string;
}) {
  return (
    <span className={large ? "bill-step large" : "bill-step"}>
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
        aria-label={label}
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

export type Preview =
  | { for: string; state: "loading" }
  | { for: string; state: "ready"; preview: ChangePreview }
  | { for: string; state: "failed"; problem: string };

/** What a change would charge, asked once the change has stopped moving: the route shares the change's rate limit. */
export function useChangePreview(
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

export function previewReady(seen?: Preview): ChangePreview | undefined {
  return seen?.state === "ready" ? seen.preview : undefined;
}

export function chargedToday(preview?: ChangePreview) {
  return preview && preview.now.amount > 0
    ? ` ${money(preview.now)} charged today.`
    : "";
}

export function PreviewWords({
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
        for what&apos;s left of this period{then ? `, then ${then}` : ""}.
      </span>
    </span>
  ) : (
    <span>
      Nothing to pay today
      <span className="soft">{then ? `, then ${then}` : ""}.</span>
    </span>
  );
}

const FEATURE_ICONS: Partial<Record<PlanFeature, ReactNode>> = {
  sync: <IconRefresh size={16} />,
  move_work: <IconSwap size={16} />,
  history_search: <IconSearch size={16} />,
  own_triggers: <IconClock size={16} />,
};

export function FeatureList({ features }: { features: PlanFeature[] }) {
  return (
    <ul className="bill-adds">
      {features.map((feature) => {
        const [name, detail] = FEATURE_WORDS[feature].split(": ");
        return (
          <li key={feature}>
            {FEATURE_ICONS[feature] ?? <IconCheck size={16} />}
            <span>
              {name}
              {detail ? (
                <small>{`${detail.charAt(0).toUpperCase()}${detail.slice(1)}`}</small>
              ) : null}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

export function CardLine({
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
    <>
      <Row
        name={card ? cardName(card) : "No card on file"}
        detail={
          card ? (
            declined ? (
              <span className="bill-bad">Declined</span>
            ) : (
              `Ends ${cardExpiry(card)}`
            )
          ) : (
            "The payment provider holds the card; we never see it."
          )
        }
        end={
          <Btn
            small
            quiet
            busy={portal.busy}
            onClick={() => void portal.open()}
          >
            {card ? "Change" : "Add"}
          </Btn>
        }
      />
      <Problem>{portal.problem}</Problem>
    </>
  );
}

const STATUS_WORDS: Record<Invoice["status"], string> = {
  paid: "Paid",
  failed: "Failed",
  refunded: "Refunded",
  disputed: "Disputed",
};

function invoiceWhat(invoice: Invoice, catalog: BillingCatalog): string {
  return invoice.lines
    .map((line) => {
      const product = productOf(catalog, line.product);
      const name = product?.name ?? line.product;
      if (product?.unit === "seat") return seatsWord(line.quantity);
      if (product?.unit === "box" && line.quantity > 1)
        return `${name}, ${line.quantity} boxes`;
      return name;
    })
    .join(" · ");
}

export function InvoiceTable({
  invoices,
  catalog,
  compact,
  empty = "No invoices yet. The first comes with the first charge.",
}: {
  invoices: Invoice[];
  catalog: BillingCatalog;
  compact?: boolean;
  empty?: string;
}) {
  const client = useClient();
  const [busyId, setBusyId] = useState<string>();
  const [problem, setProblem] = useState<string>();

  if (invoices.length === 0) return <Empty>{empty}</Empty>;

  const openReceipt = async (id: string) => {
    setBusyId(id);
    setProblem(undefined);
    try {
      const { url } = await (
        await client("get the receipt")
      ).getInvoiceReceipt(id);
      location.assign(url);
    } catch (error) {
      setProblem(`${problemText(error)} Try the receipt again.`);
      setBusyId(undefined);
    }
  };

  return (
    <>
      <Table
        label="Invoices"
        cols={
          compact
            ? "100px minmax(0, 1fr) 76px 76px 60px"
            : "120px minmax(0, 1fr) 96px 96px 72px"
        }
        heads={["Date", "What", "Status", "Amount", ""]}
      >
        {invoices.map((invoice) => {
          const what = invoiceWhat(invoice, catalog);
          const status = invoice.partlyRefunded
            ? `${money(invoice.partlyRefunded)} refunded`
            : STATUS_WORDS[invoice.status];
          const bad =
            invoice.status === "failed" || invoice.status === "disputed";
          const amount = money(invoice.total);
          return (
            <Tr key={invoice.id}>
              <Td>
                <Who
                  name={formatDay(invoice.at, "short")}
                  phone={
                    <>
                      {what} ·{" "}
                      <span className={bad ? "bill-bad" : undefined}>
                        {status}
                      </span>
                    </>
                  }
                />
              </Td>
              <Td wide>{what}</Td>
              <Td wide>
                <span className={bad ? "bill-bad" : undefined}>{status}</span>
              </Td>
              <Td wide>
                <span className="bill-amt">{amount}</span>
              </Td>
              <Td end>
                <span className="bill-amt bill-ph-amt">{amount}</span>
                <Txt
                  disabled={busyId === invoice.id}
                  onClick={() => void openReceipt(invoice.id)}
                >
                  Receipt
                </Txt>
              </Td>
            </Tr>
          );
        })}
      </Table>
      <Problem>{problem}</Problem>
    </>
  );
}

export function CancelPlanConfirm({
  sub,
  name,
  onClose,
  reload,
  show,
}: {
  sub: Subscription;
  name: string;
  onClose: () => void;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const action = useAction();
  const ends = formatDay(sub.currentPeriodEnd, "long");
  const pause = sub.seatPause;
  const resume = async () => {
    await (await client("keep your plan")).resumeSubscription(sub.id);
    reload();
  };

  return (
    <Confirm
      title={`Cancel ${name}?`}
      problem={action.problem}
      busy={action.busy}
      label={`Cancel ${name}`}
      danger
      onCancel={onClose}
      onConfirm={() =>
        void action.run(
          async () => {
            await (await client("cancel your plan")).cancelSubscription(sub.id);
            onClose();
            reload();
            if (pause)
              show(
                `${name} cancelled. The days you'd already paid are refunded.`,
              );
            else show(`${name} cancelled. It runs until ${ends}.`, resume);
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
  );
}

export function RefundConfirm({
  sub,
  catalog,
  onClose,
  reload,
  show,
}: {
  sub: Subscription;
  catalog: BillingCatalog;
  onClose: () => void;
  reload: () => void;
  show: Show;
}) {
  const client = useClient();
  const action = useAction();
  const plan = productOf(catalog, sub.product)?.name ?? sub.product;
  const bought = formatDay(sub.startedAt, "long");
  const amount = sub.refundAmount ? money(sub.refundAmount) : undefined;
  const again =
    productOf(catalog, sub.product)?.kind === "shed" ? "a Shed box" : plan;

  return (
    <Confirm
      title="Ask for a refund?"
      problem={action.problem}
      busy={action.busy}
      label={amount ? `Refund ${amount}` : "Ask for the refund"}
      onCancel={onClose}
      onConfirm={() =>
        void action.run(
          async () => {
            await (await client("ask for a refund")).refundSubscription(sub.id);
            onClose();
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
        {amount ? `, ${amount},` : ""} comes back. The payment provider takes a
        few days, and {plan} ends once it has. A refund is given once for each
        plan, so buying {again} again has none.
      </p>
    </Confirm>
  );
}

export type PlanAsk =
  { interval: BillingInterval } | { product: BillingProductId };

export function ChangePlanDialog({
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
  const title =
    "interval" in asked
      ? `Pay ${intervalWord(interval)}?`
      : `Change ${name} size`;

  return (
    <Confirm
      title={title}
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
      <div className="step-line">
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
      </div>
    </Confirm>
  );
}

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
  const client = useClient();
  const keepAction = useAction();
  const [asked, setAsked] = useState<PlanAsk>();
  const product = productOf(catalog, sub.product);
  const name = product?.name ?? sub.product;
  const refunding = !!sub.refundRequestedAt;
  const open = mayChange(sub, pays, canBuy);
  const held = heldChange(sub);
  const ends = formatDay(sub.currentPeriodEnd, "long");

  const becomes = held
    ? [
        held.product !== sub.product
          ? productOf(catalog, held.product)?.name
          : undefined,
        held.quantity !== sub.quantity
          ? countWord(held.quantity, false)
          : undefined,
        held.interval !== sub.interval
          ? intervalWord(held.interval)
          : undefined,
      ]
        .filter(Boolean)
        .join(", ")
    : undefined;

  return (
    <>
      <Row
        wrap
        name={name}
        detail={
          [
            refunding
              ? "Ends when its refund is paid"
              : held && becomes
                ? `Becomes ${becomes} from ${ends}`
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
              <Seg
                label={`How ${name} is billed`}
                value={
                  asked && "interval" in asked ? asked.interval : sub.interval
                }
                onChange={(interval) =>
                  setAsked(interval === sub.interval ? undefined : { interval })
                }
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
              <Btn
                small
                quiet
                onClick={() => setAsked({ product: sub.product })}
              >
                Change size
              </Btn>
            </>
          ) : held && pays ? (
            <Btn
              small
              quiet
              busy={keepAction.busy}
              onClick={() =>
                void keepAction.run(
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
              Keep {name}
            </Btn>
          ) : null
        }
      >
        {keepAction.problem ? <Problem>{keepAction.problem}</Problem> : null}
      </Row>
      {asked ? (
        <ChangePlanDialog
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
    </>
  );
}
