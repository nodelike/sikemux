import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import { retryLater } from "../retry.ts";
import type { BillingProvider, HeldSubscription } from "./provider.ts";

/** Something to ask of the provider, kept until it is taken. */
export type ProviderCall =
  | {
      action: "cancel_now" | "cancel_at_period_end";
      subscription: HeldSubscription;
    }
  | { action: "refund"; providerPaymentId: string };

export type Calling = Pick<BillingProvider, "name" | "cancel" | "refund">;

const BATCH = 50;

/** Calls this provider object is making now, so a call it starts while making one is not repeated. */
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
 * that fails is tried again later. The provider treats a cancel or refund made twice as one.
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
    if (busy.has(call.id)) continue;
    busy.add(call.id);
    try {
      if (call.action === "refund" && call.provider_payment_id)
        await provider.refund(call.provider_payment_id);
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
      busy.delete(call.id);
    }
  }
}
