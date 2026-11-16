import { sql, type Kysely } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import { retryLater } from "../retry.ts";
import { heldSubscription, type BillingProvider } from "./provider.ts";

/** How long after ending a deleted account's billing at once the sweeper checks the provider did. */
const RECHECK_MS = 60 * 60_000;
const BATCH = 20;

/**
 * What a deleted account still pays for: its own subscriptions and those of the orgs it is still
 * in, which after the deletion are only the ones it was alone in and which go with it.
 */
function openSubscriptionsOf(db: Kysely<Tables>, userId: string) {
  return db
    .selectFrom("subscriptions")
    .innerJoin(
      "billing_customers",
      "billing_customers.id",
      "subscriptions.customer_id",
    )
    .where("subscriptions.status", "<>", "ended")
    .where((eb) =>
      eb.or([
        eb("billing_customers.user_id", "=", userId),
        eb(
          "billing_customers.org_id",
          "in",
          eb
            .selectFrom("org_users")
            .select("org_id")
            .where("user_id", "=", userId),
        ),
      ]),
    );
}

export async function holdsOpenSubscriptions(
  db: Kysely<Tables>,
  userId: string,
): Promise<boolean> {
  const row = await openSubscriptionsOf(db, userId)
    .select("subscriptions.id")
    .executeTakeFirst();
  return row !== undefined;
}

/**
 * Ends a deleted account's subscriptions with the provider: at the period's end while it can be
 * restored, at once when it is due to be erased. The provider's events then change the rows. A
 * failure is recorded for the sweeper to retry, and alerts once it keeps failing.
 */
export async function endDeletedBilling(
  db: Kysely<Tables>,
  provider: BillingProvider | null,
  log: Logger,
  userId: string,
): Promise<void> {
  const user = await db
    .selectFrom("users")
    .select(["deleted_at", "purge_after", "billing_attempts"])
    .where("id", "=", userId)
    .executeTakeFirst();
  if (!user) return;
  const rows =
    user.deleted_at && user.purge_after
      ? await openSubscriptionsOf(db, userId)
          .selectAll("subscriptions")
          .select([
            "billing_customers.provider",
            "billing_customers.provider_customer_id",
          ])
          .execute()
      : [];
  const settle = (set: {
    billing_attempts: number;
    billing_retry_at: Date | null;
  }) => db.updateTable("users").set(set).where("id", "=", userId).execute();
  if (!rows.length || !user.purge_after)
    return void (await settle({ billing_attempts: 0, billing_retry_at: null }));

  const due = user.purge_after <= new Date();
  const when = due ? "now" : "period_end";
  try {
    for (const row of rows) {
      if (!due && row.cancel_at_period_end) continue;
      if (provider?.name !== row.provider)
        throw new Error(`no ${row.provider} payment provider is set up`);
      await provider.cancel(
        heldSubscription(row, row.provider_customer_id),
        when,
      );
      await audit(db, {
        userId,
        actor: "system",
        action: "billing.subscription_canceled",
        subject: row.id,
        detail: { when, why: "account_deleted" },
      });
    }
  } catch (error) {
    const attempts = user.billing_attempts + 1;
    await settle({
      billing_attempts: attempts,
      billing_retry_at: retryLater(
        log,
        {
          service: "The payment provider",
          what: "end a deleted account's subscriptions",
        },
        { error, userId, attempts },
      ),
    });
    return;
  }
  await settle({
    billing_attempts: 0,
    billing_retry_at: due
      ? new Date(Date.now() + RECHECK_MS)
      : user.purge_after,
  });
}

/** Ends the billing of deleted accounts whose turn has come. */
export async function sweepDeletedBilling(
  db: Kysely<Tables>,
  provider: BillingProvider | null,
  log: Logger,
): Promise<void> {
  const users = await db
    .selectFrom("users")
    .select("id")
    .where("billing_retry_at", "<=", sql<Date>`now()`)
    .orderBy("billing_retry_at")
    .limit(BATCH)
    .execute();
  for (const { id } of users) await endDeletedBilling(db, provider, log, id);
}
