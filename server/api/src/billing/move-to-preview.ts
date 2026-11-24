import { sql, type Kysely, type Transaction } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { conflict, forbidden } from "../http.ts";
import { orgRole } from "../orgs/roles.ts";
import { seatsChanged } from "../orgs/shared.ts";
import { callLater } from "./calls.ts";
import { heldSubscription } from "./provider.ts";
import { lockSeats } from "./seats.ts";
import { PADDLE_SANDBOX } from "./paddle.ts";
import { PREVIEW_SEATS, previewSeats } from "./team.ts";

/** Providers whose plans take no real money. */
const UNPAID: readonly string[] = [PADDLE_SANDBOX, "stand-in"];

/**
 * Gives an org the preview's free seats and ends its Team plan, in one step here, so no reader
 * finds it with neither. The provider is told after, and again until it takes it.
 */
export async function moveToPreview(
  db: Kysely<Tables>,
  provider: string,
  orgId: string,
  userId: string,
): Promise<boolean> {
  return db.transaction().execute(async (trx) => {
    const held = await lockSeats(trx, orgId);
    if ((await orgRole(trx, orgId, userId)) !== "owner")
      throw forbidden("Only the org's owner acts for its billing.");
    if (!UNPAID.includes(provider))
      throw conflict("Only a test plan moves to the free preview.");
    if (held > PREVIEW_SEATS)
      throw conflict(
        `The free preview has ${PREVIEW_SEATS} seats and this org uses ${held}, pending invites included. Remove some first.`,
      );
    await trx
      .updateTable("orgs")
      .set({ preview_seats: PREVIEW_SEATS })
      .where("id", "=", orgId)
      .execute();
    if (!(await previewSeats(trx, orgId)))
      throw conflict("The preview isn't open to this org.");
    await dropCheckouts(trx, provider, orgId);
    const ended = await trx
      .updateTable("subscriptions")
      .set({
        status: "ended",
        ended_at: sql<Date>`now()`,
        provider_updated_at: sql<Date>`now()`,
      })
      .where("status", "<>", "ended")
      .where("product", "=", "team")
      .where("customer_id", "in", (eb) =>
        eb
          .selectFrom("billing_customers")
          .select("id")
          .where("org_id", "=", orgId)
          .where("provider", "=", provider),
      )
      .returningAll()
      .execute();
    if (!ended.length) return false;
    const customers = await trx
      .selectFrom("billing_customers")
      .select(["id", "provider_customer_id"])
      .where("org_id", "=", orgId)
      .execute();
    await callLater(
      trx,
      provider,
      ended.map((row) => ({
        action: "cancel_now",
        subscription: heldSubscription(
          row,
          customers.find((each) => each.id === row.customer_id)
            ?.provider_customer_id ?? null,
        ),
      })),
    );
    const members = await trx
      .selectFrom("org_users")
      .select("user_id")
      .where("org_id", "=", orgId)
      .execute();
    await seatsChanged(
      trx,
      members.map((member) => member.user_id),
    );
    for (const row of ended)
      await audit(trx, {
        userId,
        actor: `user:${userId}`,
        action: "billing.subscription_canceled",
        subject: row.id,
        detail: { why: "preview" },
      });
    return true;
  });
}

/** Makes a payment on a checkout the org left open a stray, so it is cancelled and refunded, not applied. */
async function dropCheckouts(
  trx: Transaction<Tables>,
  provider: string,
  orgId: string,
) {
  const dropped = await trx
    .deleteFrom("billing_checkouts")
    .where("provider", "=", provider)
    .where("product", "=", "team")
    .where("customer_id", "in", (eb) =>
      eb
        .selectFrom("billing_customers")
        .select("id")
        .where("org_id", "=", orgId),
    )
    .returning("provider_checkout_id")
    .execute();
  if (dropped.length)
    await trx
      .insertInto("billing_strays")
      .values(
        dropped.map((row) => ({
          provider,
          provider_id: row.provider_checkout_id,
        })),
      )
      .onConflict((oc) => oc.columns(["provider", "provider_id"]).doNothing())
      .execute();
}
