import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import { retryLater } from "../retry.ts";
import type { BillingProvider, HeldSubscription } from "./provider.ts";
import { settleSeatPause } from "./seat-pause.ts";

/** Something to ask of the provider, kept until it is taken. */
export type ProviderCall =
  | {
      action: "cancel_now" | "cancel_at_period_end" | "keep_cancel";
      subscription: HeldSubscription;
    }
  | { action: "refund"; providerPaymentId: string; amount?: number };

export type Calling = Pick<
  BillingProvider,
  | "name"
  | "eventRetrySpanMs"
  | "cancel"
  | "refund"
  | "pause"
  | "resumeOn"
  | "keepPaused"
>;

const BATCH = 50;

/**
 * What this provider object is calling about now, so a call it starts while making one is not
 * repeated, and two checks of one person's seats never run at once.
 */
const running = new WeakMap<Calling, Set<string>>();

/** Records calls to make, in the transaction that decided them. */
export async function callLater(
  db: Kysely<Tables>,
  provider: string,
  calls: readonly ProviderCall[],
): Promise<void> {
  if (!calls.length) return;
  await db
    .insertInto("billing_calls")
    .values(
      calls.map((call) =>
        call.action === "refund"
          ? {
              provider,
              action: call.action,
              subscription: null,
              provider_payment_id: call.providerPaymentId,
              refund_amount: call.amount ?? null,
            }
          : {
              provider,
              action: call.action,
              subscription: JSON.stringify(call.subscription),
              provider_payment_id: null,
            },
      ),
    )
    .execute();
}

function held(stored: Record<string, unknown>): HeldSubscription {
  const subscription = stored as unknown as HeldSubscription;
  return {
    ...subscription,
    currentPeriodEnd: new Date(subscription.currentPeriodEnd),
  };
}

/**
 * Makes the calls whose turn has come, oldest first. A call the provider takes is forgotten; one
 * that fails is tried again later. The provider treats a cancel or refund made twice as one, and
 * a seat check acts only on what it finds undone.
 */
export async function makeDueCalls(
  db: Kysely<Tables>,
  provider: Calling,
  log: Logger,
): Promise<void> {
  const busy = running.get(provider) ?? new Set<string>();
  running.set(provider, busy);
  const due = await db
    .selectFrom("billing_calls")
    .selectAll()
    .where("provider", "=", provider.name)
    .where("retry_at", "<=", sql<Date>`now()`)
    .orderBy("id")
    .limit(BATCH)
    .execute();
  for (const call of due) {
    const key = call.user_id ? `user:${call.user_id}` : call.id;
    if (busy.has(key)) continue;
    busy.add(key);
    try {
      const owed = await db
        .selectFrom("billing_calls")
        .select("id")
        .where("id", "=", call.id)
        .executeTakeFirst();
      if (!owed) continue;
      if (call.action === "settle_seat_pause" && call.user_id) {
        const askAgainAt = await settleSeatPause(
          db,
          provider,
          call.user_id,
          log,
        );
        if (askAgainAt) {
          await db
            .updateTable("billing_calls")
            .set({ retry_at: askAgainAt })
            .where("id", "=", call.id)
            .execute();
          continue;
        }
      } else if (call.action === "refund" && call.provider_payment_id)
        await (call.refund_amount === null
          ? provider.refund(call.provider_payment_id)
          : provider.refund(
              call.provider_payment_id,
              Number(call.refund_amount),
            ));
      else if (call.action === "keep_cancel" && call.subscription)
        await keepCancel(db, provider, held(call.subscription));
      else if (call.subscription)
        await provider.cancel(
          held(call.subscription),
          call.action === "cancel_now" ? "now" : "period_end",
        );
      await db.deleteFrom("billing_calls").where("id", "=", call.id).execute();
    } catch (error) {
      const attempts = call.attempts + 1;
      await db
        .updateTable("billing_calls")
        .set({
          attempts,
          retry_at: retryLater(
            log,
            { service: "The payment provider", what: `take a ${call.action}` },
            { error, provider: provider.name, attempts },
          ),
        })
        .where("id", "=", call.id)
        .execute();
    } finally {
      busy.delete(key);
    }
  }
}

/**
 * Puts the person's cancel back at the provider, only while it still stands here: not taken back,
 * and the plan neither ended nor paused for a seat, whose settle carries the cancel itself. The
 * provider takes no cancel at the period's end on a paused plan. A take-back clears the cancel in
 * its own turn before it asks the provider, so the two never cross.
 */
async function keepCancel(
  db: Kysely<Tables>,
  provider: Calling,
  subscription: HeldSubscription,
) {
  await db.transaction().execute(async (trx) => {
    if (
      !(await takeCancelTurn(trx, subscription.providerSubscriptionId, false))
    )
      throw new Error("A take-back of this cancel is under way.");
    const stands = await trx
      .selectFrom("subscriptions")
      .innerJoin(
        "billing_customers",
        "billing_customers.id",
        "subscriptions.customer_id",
      )
      .select("subscriptions.id")
      .where("billing_customers.provider", "=", provider.name)
      .where(
        "subscriptions.provider_subscription_id",
        "=",
        subscription.providerSubscriptionId,
      )
      .where("subscriptions.product", "=", subscription.product)
      .where("subscriptions.cancel_asked_at", "is not", null)
      .where("subscriptions.status", "<>", "ended")
      .where("subscriptions.status", "in", ["active", "lapsed"])
      .where("subscriptions.paused_for_seat_at", "is", null)
      .executeTakeFirst();
    if (stands) await provider.cancel(subscription, "period_end");
  });
}

/**
 * Takes, until the transaction ends, the turn to change whether the provider holds a
 * subscription's cancel. A take-back waits for its turn; a re-cancel whose turn is taken tries
 * again later, since the stand-in provider applies its events while a call is under way.
 */
export async function takeCancelTurn(
  trx: Kysely<Tables>,
  providerSubscriptionId: string,
  wait = true,
) {
  const key = `cancel:${providerSubscriptionId}`;
  if (wait) {
    await sql`select pg_advisory_xact_lock(hashtextextended(${key}, 0))`.execute(
      trx,
    );
    return true;
  }
  const { rows } = await sql<{
    taken: boolean;
  }>`select pg_try_advisory_xact_lock(hashtextextended(${key}, 0)) as taken`.execute(
    trx,
  );
  return rows[0]?.taken === true;
}
