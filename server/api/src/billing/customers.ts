import type { Kysely } from "kysely";

import type { ClerkBackend } from "../account/clerk.ts";
import type { Tables } from "../db.ts";
import { ApiFailure } from "../http.ts";
import type { BillingProvider } from "./provider.ts";

/**
 * The person's own customer at the provider: the one recorded at their first checkout, else one
 * made from their verified email. A customer the provider already holds under that email is taken
 * only when no other person holds it; when one does, the email has passed on, so that customer
 * moves to its holder's current email and this person gets a new one.
 */
export async function ownCustomer(
  db: Kysely<Tables>,
  provider: BillingProvider,
  clerk: ClerkBackend | null,
  userId: string,
): Promise<string> {
  const recorded = (holder: string) =>
    db
      .selectFrom("billing_people")
      .select("provider_customer_id")
      .where("provider", "=", provider.name)
      .where("user_id", "=", holder)
      .executeTakeFirst();
  const known = await recorded(userId);
  if (known) return known.provider_customer_id;

  const emailOf = async (id: string) =>
    clerk ? await clerk.verifiedEmail(id) : null;
  const email = await emailOf(userId);
  let found = await provider.customerFor(userId, email);
  if (!found)
    throw new ApiFailure(
      409,
      "conflict",
      "Verify the email you sign in with before buying.",
    );
  if (!found.made) {
    const holder = await db
      .selectFrom("billing_people")
      .select("user_id")
      .where("provider", "=", provider.name)
      .where("provider_customer_id", "=", found.id)
      .executeTakeFirst();
    if (holder && holder.user_id !== userId) {
      const theirs = await emailOf(holder.user_id);
      if (!theirs)
        throw new ApiFailure(
          409,
          "conflict",
          "This email still pays for another account's plan. Buy once that plan has ended.",
        );
      await provider.readdress(found.id, theirs);
      found = await provider.customerFor(userId, email);
      if (!found?.made)
        throw new Error(
          "The provider kept the email on the customer it moved.",
        );
    }
  }
  await db
    .insertInto("billing_people")
    .values({
      provider: provider.name,
      user_id: userId,
      provider_customer_id: found.id,
    })
    .onConflict((oc) => oc.columns(["provider", "user_id"]).doNothing())
    .execute();
  const kept = await recorded(userId);
  if (!kept) throw new Error("The person's customer was not recorded.");
  return kept.provider_customer_id;
}
