import type { SeatPause } from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import type { Logger } from "../log.ts";
import { callLater, tryCancelTurn } from "./calls.ts";
import {
  heldSubscription,
  type BillingProvider,
  type HeldAs,
} from "./provider.ts";

const PAYMENT_WAIT_MS = 60 * 60_000;
const CANCEL_TURN_WAIT_MS = 30_000;

export type Pausing = Pick<
  BillingProvider,
  | "name"
  | "eventRetrySpanMs"
  | "pause"
  | "resumeOn"
  | "keepPaused"
  | "cancel"
  | "refund"
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
  paused_for_seat_at: Date | null;
  resume_asked_at: Date | null;
};

/** When we asked the provider to resume this pause, if we did. */
function resumeAskedThisPause(row: Held) {
  return row.resume_asked_at &&
    row.paused_for_seat_at &&
    row.resume_asked_at >= row.paused_for_seat_at
    ? row.resume_asked_at
    : null;
}

/**
 * Narrows our record of a resume asked during this pause to charges that started by now, as the
 * provider already holds the plan as running; one asked during an earlier pause is dropped.
 */
function settleResumeAsk(row: Held) {
  return resumeAskedThisPause(row)
    ? { resume_asked_for: sql<Date>`least(resume_asked_for, now())` }
    : { resume_asked_at: null, resume_asked_for: null };
}

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
          eb("kept_refund_refused_at", "is", null),
        ]),
      ]),
    )
    .execute();
}

/** Whether the provider refused to refund the kept paid time of a cancelled Pro paused for a seat. */
async function keptRefundRefused(db: Kysely<Tables>, row: Held) {
  const refused = await db
    .selectFrom("invoices")
    .select("id")
    .where("customer_id", "=", row.customer_id)
    .where("provider_subscription_id", "=", row.provider_subscription_id)
    .where("kept_refund_refused_at", "is not", null)
    .where("kept_refunded_at", "is", null)
    .executeTakeFirst();
  return refused !== undefined;
}

/**
 * Forgets a pause the provider no longer holds, as when its resume went through and the event
 * saying so was lost, takes the status the provider holds it in, and settles the plan again as
 * the plan it is. Refunds it waited on go out as for a plan that runs: a first-days refund with a
 * cancel at the period's end, a kept-time refund not at all, as no time is kept.
 */
async function forgetPause(
  db: Kysely<Tables>,
  provider: Pausing,
  userId: string,
  row: Held,
  heldAs: Exclude<HeldAs, "paused">,
  log: Logger,
): Promise<Date | null> {
  const ended = heldAs === "ended";
  const firstDaysOf = (trx: Kysely<Tables>) =>
    trx
      .selectFrom("invoices")
      .select("provider_payment_id")
      .where("customer_id", "=", row.customer_id)
      .where("provider_subscription_id", "=", row.provider_subscription_id)
      .where("status", "=", "paid")
      .where("refund_requested_at", "is not", null)
      .execute();
  if (row.cancel_at_period_end && !ended) {
    await db.transaction().execute(async (trx) => {
      await trx
        .updateTable("subscriptions")
        .set(settleResumeAsk(row))
        .where("id", "=", row.id)
        .execute();
      const resumeAsked = resumeAskedThisPause(row);
      const resumeCharges = resumeAsked
        ? await trx
            .selectFrom("invoices")
            .select("provider_payment_id")
            .where("customer_id", "=", row.customer_id)
            .where(
              "provider_subscription_id",
              "=",
              row.provider_subscription_id,
            )
            .where("status", "=", "paid")
            .where("reason", "=", "renewal")
            .where("at", ">=", resumeAsked)
            .execute()
        : [];
      await callLater(
        trx,
        provider.name,
        [...(await firstDaysOf(trx)), ...resumeCharges].map((invoice) => ({
          action: "refund" as const,
          providerPaymentId: invoice.provider_payment_id,
        })),
      );
    });
    await endNow(db, provider, userId, row, "renewed_after_cancel");
    return null;
  }
  const refunding = await db.transaction().execute(async (trx) => {
    await trx
      .updateTable("subscriptions")
      .set({
        paused_for_seat_at: null,
        paid_left: null,
        resumes_at: null,
        ...settleResumeAsk(row),
      })
      .where("id", "=", row.id)
      .execute();
    await trx
      .updateTable("subscriptions")
      .set({
        status: heldAs,
        ...(ended && { ended_at: sql<Date>`now()` }),
      })
      .where("id", "=", row.id)
      .where("status", "<>", "ended")
      .execute();
    const dropped = await trx
      .updateTable("invoices")
      .set({ kept_refund: null, kept_refund_check_at: null })
      .where("customer_id", "=", row.customer_id)
      .where("provider_subscription_id", "=", row.provider_subscription_id)
      .where("kept_refund", "is not", null)
      .where("kept_refunded_at", "is", null)
      .where("kept_refund_refused_at", "is", null)
      .executeTakeFirst();
    if (dropped.numUpdatedRows)
      log.error(
        { subscription: row.id },
        "a Pro paused for a seat was running again at the payment provider before its kept time was refunded; the refund was not asked",
      );
    const firstDays = await firstDaysOf(trx);
    if (firstDays.length)
      await callLater(trx, provider.name, [
        ...(ended
          ? []
          : [
              {
                action: "cancel_at_period_end" as const,
                subscription: heldSubscription(row, row.provider_customer_id),
              },
            ]),
        ...firstDays.map((invoice) => ({
          action: "refund" as const,
          providerPaymentId: invoice.provider_payment_id,
        })),
      ]);
    await appendEvents(trx, [{ userId, type: "plan.changed" }]);
    return firstDays.length > 0;
  });
  return refunding || ended ? null : settleSeatPause(db, provider, userId, log);
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
 * Gives up a pause the provider never took. A cancel at the period's end the pause took off at the
 * provider is put back there.
 */
async function dropPause(
  db: Kysely<Tables>,
  provider: Pausing,
  row: Held,
  since: Date,
  putBackCancel: boolean,
) {
  await db.transaction().execute(async (trx) => {
    const dropped = await trx
      .updateTable("subscriptions")
      .set({ paused_for_seat_at: null })
      .where("id", "=", row.id)
      .where("paused_for_seat_at", "=", since)
      .where("paid_left", "is", null)
      .where("resumes_at", "is", null)
      .executeTakeFirst();
    if (dropped.numUpdatedRows && putBackCancel && row.cancel_at_period_end)
      await callLater(trx, provider.name, [
        {
          action: "keep_cancel",
          subscription: heldSubscription(row, row.provider_customer_id),
        },
      ]);
  });
  return null;
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
    const attempt = await tryCancelTurn(
      row.provider_subscription_id,
      async () => {
        const current = await proOf(db, userId, provider.name);
        if (
          current?.id !== row.id ||
          current.paused_for_seat_at?.getTime() !== since.getTime() ||
          current.paid_left !== null ||
          current.resumes_at !== null
        )
          return { again: true as const };
        const held = heldSubscription(current, current.provider_customer_id);
        const asked =
          current.seat_pause_asked_for?.getTime() === since.getTime();
        if (!seated) {
          const heldAs = asked ? await provider.keepPaused(held) : null;
          if (heldAs !== "paused")
            return {
              done: await dropPause(
                db,
                provider,
                current,
                since,
                !deleted && (heldAs === "active" || heldAs === "lapsed"),
              ),
            };
        }
        if (
          !asked ||
          (current.cancel_at_period_end && !current.cancel_asked_at)
        )
          await db
            .updateTable("subscriptions")
            .set({
              seat_pause_asked_for: since,
              ...(current.cancel_at_period_end && {
                cancel_asked_at: sql<Date>`coalesce(cancel_asked_at, now())`,
              }),
            })
            .where("id", "=", row.id)
            .execute();
        return { current, paused: await provider.pause(held, asked) };
      },
    );
    const step = attempt.taken
      ? attempt.value
      : { done: new Date(Date.now() + CANCEL_TURN_WAIT_MS) };
    if ("again" in step) return settleSeatPause(db, provider, userId, log);
    if ("done" in step) return step.done;
    const { current, paused } = step;
    if ("askAgainAt" in paused) return paused.askAgainAt;
    if ("cancelScheduled" in paused) {
      await db
        .updateTable("subscriptions")
        .set({ cancel_at_period_end: true })
        .where("id", "=", row.id)
        .where("paused_for_seat_at", "=", since)
        .where("paid_left", "is", null)
        .where("resumes_at", "is", null)
        .execute();
      return settleSeatPause(db, provider, userId, log);
    }
    if ("refused" in paused)
      return dropPause(
        db,
        provider,
        current,
        since,
        paused.refused === "not_active",
      );
    await record(
      db,
      userId,
      row.id,
      (trx) =>
        trx
          .updateTable("subscriptions")
          .set({
            paid_left: sql<string>`greatest(${paused.paidUntil}::timestamptz - now(), interval '0')`,
            provider_updated_at: sql<Date>`greatest(provider_updated_at, ${paused.pausedAt}::timestamptz)`,
          })
          .where("id", "=", row.id)
          .where("cancel_at_period_end", "=", current.cancel_at_period_end)
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
    const kept = await provider.keepPaused(held);
    if (kept !== "paused")
      return forgetPause(db, provider, userId, row, kept, log);
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
            resume_asked_at: null,
            resume_asked_for: null,
          })
          .where("id", "=", row.id)
          .where("resumes_at", "is not", null)
          .executeTakeFirst(),
      { again: true },
    );
    return settleSeatPause(db, provider, userId, log);
  }

  if (refunds.length) {
    const kept = await provider.keepPaused(held);
    if (kept !== "paused")
      return forgetPause(db, provider, userId, row, kept, log);
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
      .execute((trx) =>
        askKeptRefunds(trx, id, provider.eventRetrySpanMs, log),
      );
    if (asked === "awaiting_payment")
      return new Date(Date.now() + PAYMENT_WAIT_MS);
    if (typeof asked === "number" && asked)
      return settleSeatPause(db, provider, userId, log);
    if (asked !== "no_payment" && !(await keptRefundRefused(db, row))) {
      await endNow(db, provider, userId, row, "paused");
      return null;
    }
    if (seated || !row.runs_out_at) return null;
    await record(
      db,
      userId,
      row.id,
      (trx) =>
        trx
          .updateTable("subscriptions")
          .set({ resumes_at: row.runs_out_at, paid_left: null })
          .where("id", "=", id)
          .where("paid_left", "is not", null)
          .executeTakeFirst(),
      { endsAt: row.runs_out_at.toISOString() },
    );
    return settleSeatPause(db, provider, userId, log);
  }
  if (row.cancel_at_period_end && row.resumes_at) {
    const kept = await provider.keepPaused(held);
    if (kept !== "paused")
      return forgetPause(db, provider, userId, row, kept, log);
    if (row.resumes_at.getTime() > Date.now()) return row.resumes_at;
    await endNow(db, provider, userId, row, "kept_time_used");
    return null;
  }
  if (seated) return null;

  const at = row.resumes_at ?? row.runs_out_at;
  if (!at) return null;
  if (!row.resumes_at)
    await db
      .updateTable("subscriptions")
      .set({ resume_asked_at: sql<Date>`now()`, resume_asked_for: at })
      .where("id", "=", row.id)
      .execute();
  const resumed = await provider.resumeOn(held, at);
  if (resumed !== "paused")
    return forgetPause(db, provider, userId, row, resumed, log);
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
 * Answers how many payments are refunded, or that the current period's payment is not in yet:
 * the kept time is then that payment's, and asking of an earlier one would refund it twice. A
 * payment the provider has not sent by the end of its time for resending an event never will, as
 * for a period paid wholly from credit: the plan keeps its time unrefunded, as when the refund is
 * refused, and staff are told.
 */
export async function askKeptRefunds(
  trx: Transaction<Tables>,
  subscriptionId: string,
  paymentWaitMs: number,
  log: Logger,
): Promise<number | "awaiting_payment" | "no_payment"> {
  const row = await trx
    .selectFrom("subscriptions")
    .select([
      "customer_id",
      "provider_subscription_id",
      "current_period_end",
      "kept_without_refund_at",
      sql<Date>`current_period_end - case interval when 'year' then interval '1 year' else interval '1 month' end - interval '3 days'`.as(
        "period_started_before",
      ),
      sql<boolean>`now() > current_period_end - case interval when 'year' then interval '1 year' else interval '1 month' end + ${paymentWaitMs}::double precision * interval '1 millisecond'`.as(
        "payment_overdue",
      ),
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
  if (row.kept_without_refund_at) return "no_payment";
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
  const newest = await ofPlan
    .select("at")
    .orderBy("at", "desc")
    .limit(1)
    .executeTakeFirst();
  if (!started || !newest || newest.at < row.period_started_before) {
    if (!row.payment_overdue) return "awaiting_payment";
    await trx
      .updateTable("subscriptions")
      .set({ kept_without_refund_at: sql<Date>`now()` })
      .where("id", "=", subscriptionId)
      .execute();
    log.error(
      {
        subscription: row.provider_subscription_id,
        fingerprint: ["kept_without_refund", subscriptionId],
      },
      "a Pro cancelled while paused for a seat keeps its paid time unrefunded: the payment for its current period never came from the payment provider, so check there what was paid",
    );
    return "no_payment";
  }
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
      .set({
        kept_refund: amount,
        kept_refund_check_at: sql<Date>`now() + interval '3 days'`,
      })
      .where("id", "=", invoice.id)
      .execute();
    asked += 1;
  }
  return asked;
}

/** Tells staff, once a day, of each refund of kept paid time the provider has not answered in days. */
export async function alertWaitingKeptRefunds(db: Kysely<Tables>, log: Logger) {
  const waiting = await db
    .updateTable("invoices")
    .set({ kept_refund_check_at: sql<Date>`now() + interval '1 day'` })
    .where("kept_refund_check_at", "<=", sql<Date>`now()`)
    .where("kept_refunded_at", "is", null)
    .where("kept_refund_refused_at", "is", null)
    .where("status", "=", "paid")
    .returning(["id", "provider_payment_id", "provider_subscription_id"])
    .execute();
  for (const invoice of waiting)
    log.error(
      {
        payment: invoice.provider_payment_id,
        subscription: invoice.provider_subscription_id,
        fingerprint: ["kept_refund_unanswered", invoice.id],
      },
      "the refund of a paused Pro's kept paid time has had no answer from the payment provider for days: check it there",
    );
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
