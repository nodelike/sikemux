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
  const ran = await tryCancelTurn(
    subscription.providerSubscriptionId,
    async () => {
      const stands = await db
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
    },
  );
  if (!ran.taken) throw new Error("A take-back of this cancel is under way.");
}

const cancelTurns = new Map<string, Promise<void>>();
const cancelTurnWaiters = new Map<string, number>();

/**
 * Runs `work` once no other cancel, take-back or seat pause is running for this subscription,
 * holding no database connection while it waits or talks to the provider.
 */
export async function withCancelTurn<T>(
  providerSubscriptionId: string,
  work: () => Promise<T>,
): Promise<T> {
  const prev = cancelTurns.get(providerSubscriptionId);
  if (prev)
    cancelTurnWaiters.set(
      providerSubscriptionId,
      (cancelTurnWaiters.get(providerSubscriptionId) ?? 0) + 1,
    );
  let release = () => {};
  const next = new Promise<void>((resolve) => {
    release = resolve;
  });
  cancelTurns.set(providerSubscriptionId, next);
  try {
    if (prev) {
      try {
        await prev;
      } finally {
        const left = (cancelTurnWaiters.get(providerSubscriptionId) ?? 1) - 1;
        if (left > 0) cancelTurnWaiters.set(providerSubscriptionId, left);
        else cancelTurnWaiters.delete(providerSubscriptionId);
      }
    }
    return await work();
  } finally {
    release();
    if (cancelTurns.get(providerSubscriptionId) === next)
      cancelTurns.delete(providerSubscriptionId);
  }
}

/** Runs `work` if no cancel turn is currently held for this subscription, without waiting. */
export async function tryCancelTurn<T>(
  providerSubscriptionId: string,
  work: () => Promise<T>,
): Promise<{ taken: true; value: T } | { taken: false }> {
  if (cancelTurns.has(providerSubscriptionId)) return { taken: false };
  let release = () => {};
  const next = new Promise<void>((resolve) => {
    release = resolve;
  });
  cancelTurns.set(providerSubscriptionId, next);
  try {
    return { taken: true, value: await work() };
  } finally {
    release();
    if (cancelTurns.get(providerSubscriptionId) === next)
      cancelTurns.delete(providerSubscriptionId);
  }
}

export function cancelTurnHasWaiter(providerSubscriptionId?: string): boolean {
  if (providerSubscriptionId !== undefined)
    return (cancelTurnWaiters.get(providerSubscriptionId) ?? 0) > 0;
  for (const count of cancelTurnWaiters.values()) if (count > 0) return true;
  return false;
}
