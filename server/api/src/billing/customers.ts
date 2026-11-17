import type { Kysely } from "kysely";

import type { ClerkBackend } from "../account/clerk.ts";
import type { Tables } from "../db.ts";
import { ApiFailure } from "../http.ts";
import type { Logger } from "../log.ts";
import type { BillingProvider } from "./provider.ts";

const stillPays = () =>
  new ApiFailure(
    409,
    "conflict",
    "This email still pays for another account's plan. Buy once that plan has ended.",
  );

/** People who traded emails: each one's customer waits for the email another's holds. */
class Traded extends ApiFailure {
  constructor() {
    super(409, "conflict", stillPays().message);
  }
}

/**
 * People's customers at the provider, each kept at the email its person signs in with. An email
 * that passed from one person to another moves on with them: the customer holding it moves to its
 * own person's current email first.
 */
function customersOf(
  db: Kysely<Tables>,
  provider: BillingProvider,
  clerk: ClerkBackend | null,
) {
  const recorded = async (userId: string) =>
    (
      await db
        .selectFrom("billing_people")
        .select("provider_customer_id")
        .where("provider", "=", provider.name)
        .where("user_id", "=", userId)
        .executeTakeFirst()
    )?.provider_customer_id;
  const holderOf = async (customerId: string) =>
    (
      await db
        .selectFrom("billing_people")
        .select("user_id")
        .where("provider", "=", provider.name)
        .where("provider_customer_id", "=", customerId)
        .executeTakeFirst()
    )?.user_id;
  const emailOf = async (userId: string) =>
    clerk ? await clerk.verifiedEmail(userId) : null;

  const handOn = async (customerId: string, seen: string[]) => {
    const holder = await holderOf(customerId);
    if (holder && seen.includes(holder)) throw new Traded();
    if (!holder) throw stillPays();
    const theirs = await emailOf(holder);
    if (!theirs) throw stillPays();
    await moveTo(customerId, theirs, [...seen, holder]);
  };
  const moveTo = async (customerId: string, email: string, seen: string[]) => {
    const heldBy = await provider.readdress(customerId, email);
    if (!heldBy) return;
    await handOn(heldBy, seen);
    if (await provider.readdress(customerId, email))
      throw new Error("The provider kept the email on the customer it moved.");
  };

  return { recorded, holderOf, emailOf, handOn, moveTo };
}

/**
 * The person's own customer at the provider: the one recorded at their first checkout, moved to
 * their current email, else one made from that email. A customer the provider already holds under
 * the email is taken only when no other person holds it.
 */
export async function ownCustomer(
  db: Kysely<Tables>,
  provider: BillingProvider,
  clerk: ClerkBackend | null,
  log: Logger,
  userId: string,
): Promise<string> {
  const customers = customersOf(db, provider, clerk);
  const email = await customers.emailOf(userId);
  const known = await customers.recorded(userId);
  if (known) {
    if (email) await follow(customers, log, known, email, userId);
    return known;
  }

  let found = await provider.customerFor(userId, email);
  if (!found)
    throw new ApiFailure(
      409,
      "conflict",
      "Verify the email you sign in with before buying.",
    );
  if (!found.made) {
    const holder = await customers.holderOf(found.id);
    if (holder && holder !== userId) {
      await customers.handOn(found.id, [userId]);
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
  const kept = await customers.recorded(userId);
  if (!kept) throw new Error("The person's customer was not recorded.");
  return kept;
}

/** Moves the person's customer, if they have one, to the email they now sign in with. */
export async function followAddress(
  db: Kysely<Tables>,
  provider: BillingProvider,
  clerk: ClerkBackend,
  log: Logger,
  userId: string,
): Promise<void> {
  const customers = customersOf(db, provider, clerk);
  const known = await customers.recorded(userId);
  if (!known) return;
  const email = await customers.emailOf(userId);
  if (email) await follow(customers, log, known, email, userId);
}

/** A customer whose new email another account still pays with keeps its old one until that ends. */
async function follow(
  customers: ReturnType<typeof customersOf>,
  log: Logger,
  customerId: string,
  email: string,
  userId: string,
) {
  try {
    await customers.moveTo(customerId, email, [userId]);
  } catch (error) {
    if (!(error instanceof ApiFailure)) throw error;
    log.info(
      { userId, customerId },
      error instanceof Traded
        ? "the customer keeps its email: people traded emails, and each customer at the provider still holds another's"
        : "the customer keeps its email: another account still pays with the new one",
    );
  }
}
