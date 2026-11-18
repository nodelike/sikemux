import type { SeatPause } from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import type { Logger } from "../log.ts";
import { callLater } from "./calls.ts";
import { heldSubscription, type BillingProvider } from "./provider.ts";

export type Pausing = Pick<
  BillingProvider,
  "name" | "pause" | "resumeOn" | "keepPaused" | "cancel" | "refund"
>;

/**
 * Asks for each person's seats to be checked against their own Pro once this transaction commits.
 * Only people paying for Pro themselves get a check.
 */
export async function checkSeatPauses(
  trx: Transaction<Tables>,
  userIds: readonly string[],
): Promise<void> {
  if (!userIds.length) return;
  await trx
    .insertInto("billing_calls")
    .columns(["provider", "action", "user_id"])
    .expression(
      trx
        .selectFrom("subscriptions")
        .innerJoin(
          "billing_customers",
          "billing_customers.id",
          "subscriptions.customer_id",
        )
        .select([
          "billing_customers.provider",
          sql.lit("settle_seat_pause").as("action"),
          "billing_customers.user_id",
        ])
        .where("billing_customers.user_id", "in", userIds)
        .where("subscriptions.product", "=", "pro")
        .where("subscriptions.status", "<>", "ended"),
    )
    .execute();
}

/**
 * The orgs whose Team plan includes Pro for this person through their seat. A lapsed plan still
 * does, so a renewal that is late leaves their own Pro paused.
 */
export async function seatsIncludingPro(
  db: Kysely<Tables>,
  userId: string,
): Promise<{ id: string; name: string }[]> {
  return db
    .selectFrom("org_users")
    .innerJoin("orgs", "orgs.id", "org_users.org_id")
    .innerJoin(
      "billing_customers",
      "billing_customers.org_id",
      "org_users.org_id",
    )
    .innerJoin(
      "subscriptions",
      "subscriptions.customer_id",
      "billing_customers.id",
    )
    .select(["orgs.id", "orgs.name"])
    .where("org_users.user_id", "=", userId)
    .where("subscriptions.product", "=", "team")
    .where("subscriptions.status", "in", ["active", "lapsed"])
    .distinct()
    .orderBy("orgs.name")
    .execute();
}

async function record(
  db: Kysely<Tables>,
  userId: string,
  subscriptionId: string,
  change: (trx: Transaction<Tables>) => Promise<{ numUpdatedRows: bigint }>,
  detail: Record<string, unknown>,
) {
  await db.transaction().execute(async (trx) => {
    if (!(await change(trx)).numUpdatedRows) return;
    await audit(trx, {
      userId,
      actor: "system",
      action: "billing.subscription_paused",
      subject: subscriptionId,
      detail: { why: "seat", ...detail },
    });
    await appendEvents(trx, [{ userId, type: "plan.changed" }]);
  });
}

type Held = Parameters<typeof heldSubscription>[0] & {
  id: string;
  customer_id: string;
  provider_customer_id: string | null;
};

/** A person's own Pro that has not ended, with how long its kept paid time lasts from now. */
function proOf(db: Kysely<Tables>, userId: string, provider: string) {
  return db
    .selectFrom("subscriptions")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "subscriptions.customer_id",
    )
    .selectAll("subscriptions")
    .select([
      "billing_customers.provider_customer_id",
      sql<Date | null>`now() + subscriptions.paid_left`.as("runs_out_at"),
    ])
    .where("billing_customers.user_id", "=", userId)
    .where("billing_customers.provider", "=", provider)
    .where("subscriptions.product", "=", "pro")
    .where("subscriptions.status", "<>", "ended")
    .executeTakeFirst();
}

/**
 * The refunds a Pro paused for a seat waits on: the plan's first-days refund, asked while it was
 * paused, and the refund of its kept paid time, asked when it was cancelled.
 */
function waitingRefunds(db: Kysely<Tables>, row: Held) {
  return db
    .selectFrom("invoices")
    .select(["provider_payment_id", "kept_refund", "refund_requested_at"])
    .where("customer_id", "=", row.customer_id)
    .where("provider_subscription_id", "=", row.provider_subscription_id)
    .where("status", "=", "paid")
    .where((eb) =>
      eb.or([
        eb("refund_requested_at", "is not", null),
        eb.and([
          eb("kept_refund", "is not", null),
          eb("kept_refunded_at", "is", null),
        ]),
      ]),
    )
    .execute();
}

/**
 * Forgets a pause the provider no longer holds, as when its resume went through and the event
 * saying so was lost. Refunds it waited on go out as for a plan that runs: a first-days refund
 * with a cancel at the period's end, a kept-time refund not at all, as no time is kept.
 */
async function forgetPause(
  db: Kysely<Tables>,
  provider: string,
  row: Held,
  log: Logger,
): Promise<null> {
  await db.transaction().execute(async (trx) => {
    await trx
      .updateTable("subscriptions")
      .set({ paused_for_seat_at: null, paid_left: null, resumes_at: null })
      .where("id", "=", row.id)
      .execute();
    const dropped = await trx
      .updateTable("invoices")
      .set({ kept_refund: null })
      .where("customer_id", "=", row.customer_id)
      .where("provider_subscription_id", "=", row.provider_subscription_id)
      .where("kept_refund", "is not", null)
      .where("kept_refunded_at", "is", null)
      .executeTakeFirst();
    if (dropped.numUpdatedRows)
      log.error(
        { subscription: row.id },
        "a Pro paused for a seat was running again at the payment provider before its kept time was refunded; the refund was not asked",
      );
    const firstDays = await trx
      .selectFrom("invoices")
      .select("provider_payment_id")
      .where("customer_id", "=", row.customer_id)
      .where("provider_subscription_id", "=", row.provider_subscription_id)
      .where("status", "=", "paid")
      .where("refund_requested_at", "is not", null)
      .execute();
    if (firstDays.length)
      await callLater(trx, provider, [
        {
          action: "cancel_at_period_end",
          subscription: heldSubscription(row, row.provider_customer_id),
        },
        ...firstDays.map((invoice) => ({
          action: "refund" as const,
          providerPaymentId: invoice.provider_payment_id,
        })),
      ]);
  });
  return null;
}

/** Ends a paused Pro at once, which is the only way the provider ends a paused subscription. */
async function endNow(
  db: Kysely<Tables>,
  provider: Pick<BillingProvider, "cancel">,
  userId: string,
  row: Held,
  why: string,
) {
  await provider.cancel(heldSubscription(row, row.provider_customer_id), "now");
  await db.transaction().execute(async (trx) => {
    const ended = await trx
      .updateTable("subscriptions")
      .set({ status: "ended", ended_at: sql<Date>`now()` })
      .where("id", "=", row.id)
      .where("status", "<>", "ended")
      .executeTakeFirst();
    if (!ended.numUpdatedRows) return;
    await audit(trx, {
      userId,
      actor: "system",
      action: "billing.subscription_canceled",
      subject: row.id,
      detail: { when: "now", why },
    });
    await appendEvents(trx, [{ userId, type: "plan.changed" }]);
  });
}

/**
 * Makes a person's own Pro match their seats: paused, with its paid time kept, while a seat
 * includes Pro; once none does, Pro runs on that kept time and the provider bills again only when
 * it is used up. Cancelled while its time is kept, the kept time is refunded and the plan ends once
 * the refund is paid; cancelled while that time runs, it ends when the time runs out. A refund it
 * waits on holds the clock, so a refund refused costs no time. A pause made anywhere else is never
 * taken over. Answers when to check again, as when the provider asked to wait before a renewal.
 */
export async function settleSeatPause(
  db: Kysely<Tables>,
  provider: Pausing,
  userId: string,
  log: Logger,
): Promise<Date | null> {
  const user = await db
    .selectFrom("users")
    .select("deleted_at")
    .where("id", "=", userId)
    .executeTakeFirst();
  if (!user) return null;
  const deleted = user.deleted_at !== null;
  let row = await proOf(db, userId, provider.name);
  if (!row) return null;
  const seated = !deleted && (await seatsIncludingPro(db, userId)).length > 0;

  if (!row.paused_for_seat_at) {
    if (!seated || row.status !== "active") return null;
    const marked = await db
      .updateTable("subscriptions")
      .set({ paused_for_seat_at: sql<Date>`date_trunc('milliseconds', now())` })
      .where("id", "=", row.id)
      .where("paused_for_seat_at", "is", null)
      .where("status", "=", "active")
      .returning("paused_for_seat_at")
      .executeTakeFirst();
    if (!marked) return null;
    row = { ...row, paused_for_seat_at: marked.paused_for_seat_at };
  }
  const since = row.paused_for_seat_at;
  if (!since) return null;
  const held = heldSubscription(row, row.provider_customer_id);

  if (row.paid_left === null && row.resumes_at === null) {
    const paused = await provider.pause(held, since);
    if ("askAgainAt" in paused) return paused.askAgainAt;
    if ("refused" in paused) {
      await db
        .updateTable("subscriptions")
        .set({ paused_for_seat_at: null })
        .where("id", "=", row.id)
        .where("paused_for_seat_at", "=", since)
        .where("paid_left", "is", null)
        .where("resumes_at", "is", null)
        .execute();
      return null;
    }
    await record(
      db,
      userId,
      row.id,
      (trx) =>
        trx
          .updateTable("subscriptions")
          .set((eb) => ({
            paid_left: sql<string>`greatest(${paused.paidUntil}::timestamptz - now(), interval '0')`,
            cancel_at_period_end: paused.wasCancelling
              ? true
              : eb.ref("cancel_at_period_end"),
            provider_updated_at: sql<Date>`greatest(provider_updated_at, ${paused.pausedAt}::timestamptz)`,
          }))
          .where("id", "=", row.id)
          .where("paused_for_seat_at", "=", since)
          .where("paid_left", "is", null)
          .where("resumes_at", "is", null)
          .executeTakeFirst(),
      {
        paidUntil: paused.paidUntil.toISOString(),
        cancelling: paused.wasCancelling,
      },
    );
    return settleSeatPause(db, provider, userId, log);
  }

  const refunds = await waitingRefunds(db, row);
  if (row.resumes_at && (seated || deleted || refunds.length)) {
    if (!(await provider.keepPaused(held)))
      return forgetPause(db, provider.name, row, log);
    await record(
      db,
      userId,
      row.id,
      (trx) =>
        trx
          .updateTable("subscriptions")
          .set({
            paid_left: sql<string>`greatest(resumes_at - now(), interval '0')`,
            resumes_at: null,
          })
          .where("id", "=", row.id)
          .where("resumes_at", "is not", null)
          .executeTakeFirst(),
      { again: true },
    );
    return settleSeatPause(db, provider, userId, log);
  }

  if (refunds.length) {
    if (!(await provider.keepPaused(held)))
      return forgetPause(db, provider.name, row, log);
    for (const refund of refunds)
      await (refund.kept_refund === null
        ? provider.refund(refund.provider_payment_id)
        : provider.refund(
            refund.provider_payment_id,
            Number(refund.kept_refund),
          ));
    return null;
  }
  if (deleted) return null;

  if (row.cancel_at_period_end && row.paid_left !== null) {
    const id = row.id;
    const asked = await db
      .transaction()
      .execute((trx) => askKeptRefunds(trx, id));
    if (asked) return settleSeatPause(db, provider, userId, log);
    await endNow(db, provider, userId, row, "paused");
    return null;
  }
  if (row.cancel_at_period_end && row.resumes_at) {
    if (!(await provider.keepPaused(held)))
      return forgetPause(db, provider.name, row, log);
    if (row.resumes_at.getTime() > Date.now()) return row.resumes_at;
    await endNow(db, provider, userId, row, "kept_time_used");
    return null;
  }
  if (seated) return null;

  const at = row.resumes_at ?? row.runs_out_at;
  if (!at) return null;
  if (!(await provider.resumeOn(held, at)))
    return forgetPause(db, provider.name, row, log);
  if (row.resumes_at) return null;
  await db.transaction().execute(async (trx) => {
    const set = await trx
      .updateTable("subscriptions")
      .set({ resumes_at: at, paid_left: null })
      .where("id", "=", row.id)
      .where("resumes_at", "is", null)
      .executeTakeFirst();
    if (!set.numUpdatedRows) return;
    await audit(trx, {
      userId,
      actor: "system",
      action: "billing.subscription_resume_set",
      subject: row.id,
      detail: { at: at.toISOString() },
    });
    await appendEvents(trx, [{ userId, type: "plan.changed" }]);
  });
  const after = await db
    .selectFrom("users")
    .select("deleted_at")
    .where("id", "=", userId)
    .executeTakeFirst();
  return after?.deleted_at ? settleSeatPause(db, provider, userId, log) : null;
}

/**
 * Asks for the refund of the paid time a cancelled Pro paused for a seat still keeps: of each
 * payment in its last period, the share the kept time is of what that payment paid for. The
 * amounts are marked on the payments, and the time stays kept until the provider pays them.
 * Answers how many payments are refunded.
 */
export async function askKeptRefunds(
  trx: Transaction<Tables>,
  subscriptionId: string,
): Promise<number> {
  const row = await trx
    .selectFrom("subscriptions")
    .select([
      "customer_id",
      "provider_subscription_id",
      "current_period_end",
      sql<
        number | null
      >`extract(epoch from coalesce(paid_left, greatest(resumes_at - now(), interval '0')))::float8`.as(
        "kept_seconds",
      ),
    ])
    .where("id", "=", subscriptionId)
    .where("paused_for_seat_at", "is not", null)
    .forUpdate()
    .executeTakeFirst();
  if (!row?.kept_seconds) return 0;
  const ofPlan = trx
    .selectFrom("invoices")
    .where("customer_id", "=", row.customer_id)
    .where("provider_subscription_id", "=", row.provider_subscription_id)
    .where("status", "=", "paid");
  const started = await ofPlan
    .select("at")
    .where("reason", "in", ["first_payment", "renewal"])
    .orderBy("at", "desc")
    .limit(1)
    .executeTakeFirst();
  if (!started) return 0;
  const paid = await ofPlan
    .select(["id", "amount", "at"])
    .where("at", ">=", started.at)
    .where("kept_refund", "is", null)
    .where("refund_requested_at", "is", null)
    .execute();
  let asked = 0;
  for (const invoice of paid) {
    const span =
      (row.current_period_end.getTime() - invoice.at.getTime()) / 1000;
    const share = span > 0 ? Math.min(1, row.kept_seconds / span) : 1;
    const amount = Math.floor(Number(invoice.amount) * share);
    if (amount <= 0) continue;
    await trx
      .updateTable("invoices")
      .set({ kept_refund: amount })
      .where("id", "=", invoice.id)
      .execute();
    asked += 1;
  }
  return asked;
}

/** Where a Pro paused for a seat stands, as a plan shows it. */
export async function seatPauseOf(
  db: Kysely<Tables>,
  row: {
    customer_id: string;
    status: string;
    paused_for_seat_at: Date | null;
    resumes_at: Date | null;
  },
): Promise<SeatPause | undefined> {
  if (!row.paused_for_seat_at || row.status !== "paused") return undefined;
  const customer = await db
    .selectFrom("billing_customers")
    .select("user_id")
    .where("id", "=", row.customer_id)
    .executeTakeFirst();
  const orgs = customer?.user_id
    ? await seatsIncludingPro(db, customer.user_id)
    : [];
  return {
    orgs: orgs.map((org) => org.name),
    ...(row.resumes_at ? { resumesAt: row.resumes_at.toISOString() } : {}),
  };
}

/** Why a person with a seat cannot buy Pro of their own. */
export function seatIncludesPro(orgs: readonly { name: string }[]): string {
  const names = orgs.map((org) => org.name);
  const last = names.pop();
  return names.length
    ? `Your seats in ${names.join(", ")} and ${last} include Pro.`
    : `Your seat in ${last} includes Pro.`;
}
