import type { SignInRemoval } from "@sikemux/protocol";
import type { Kysely } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import { workAddressRefusal } from "./admission.ts";
import type { ClerkBackend, SignIns } from "./clerk.ts";
import { domainOf } from "./proofs.ts";

/**
 * Takes off the account every address added after it was admitted that is a work address, with
 * the connected account that brought it, so the work Google stops signing in too. Nothing the
 * account was admitted with goes, and neither does its last way to sign in.
 */
export async function stripWorkSignIns(
  db: Kysely<Tables>,
  clerk: ClerkBackend,
  proofKey: Buffer | null,
  log: Logger,
  userId: string,
): Promise<void> {
  const user = await db
    .selectFrom("users")
    .select("admitted_at")
    .where("id", "=", userId)
    .executeTakeFirst();
  const admittedAt = user?.admitted_at;
  if (!admittedAt) return;
  const signIns = await clerk.signIns(userId);
  if (!signIns) return;

  for (const address of signIns.addresses) {
    if (address.addedAt <= admittedAt) continue;
    const refusal = await workAddressRefusal(
      db,
      proofKey,
      address.email,
      address.addedAt,
    );
    if (!refusal) continue;
    const domain = refusal.domain;
    const brought = signIns.externalAccounts.filter(
      (account) => account.email.toLowerCase() === address.email.toLowerCase(),
    );
    if (waysLeft(signIns, address.id, brought) === 0) {
      log.warn(
        { userId, domain },
        "left a work address: it is the last way to sign in",
      );
      continue;
    }
    const primary =
      signIns.primaryAddressId === address.id
        ? signIns.addresses.find(
            (other) =>
              other.id !== address.id &&
              other.verified &&
              other.addedAt <= admittedAt,
          )
        : null;
    if (primary === undefined) {
      log.warn(
        { userId, domain },
        "left a work address: no earlier address to make primary",
      );
      continue;
    }
    await db.transaction().execute(async (trx) => {
      const recorded = await trx
        .insertInto("sign_in_removals")
        .values({
          user_id: userId,
          address_id: address.id,
          domain,
          org_name: refusal.why === "domain" ? refusal.orgName : null,
          provider: brought[0]?.provider ?? null,
        })
        .onConflict((oc) => oc.columns(["user_id", "address_id"]).doNothing())
        .returning("id")
        .executeTakeFirst();
      if (recorded)
        await audit(trx, {
          userId,
          actor: "system",
          action: "account.work_sign_in_removed",
          detail: { domain, provider: brought[0]?.provider ?? null },
        });
    });
    if (primary) {
      await clerk.setPrimaryAddress(userId, primary.id);
      signIns.primaryAddressId = primary.id;
    }
    for (const account of brought)
      await clerk.removeExternalAccount(userId, account.id);
    await clerk.removeAddress(address.id);
    signIns.addresses = signIns.addresses.filter((a) => a.id !== address.id);
    signIns.externalAccounts = signIns.externalAccounts.filter(
      (account) => !brought.includes(account),
    );
    log.info({ userId, domain }, "took a work address off a sign-in");
  }
}

function waysLeft(
  signIns: SignIns,
  addressId: string,
  brought: SignIns["externalAccounts"],
) {
  const accounts = signIns.externalAccounts.filter((a) => !brought.includes(a));
  const addresses = signIns.addresses.filter(
    (a) => a.id !== addressId && a.verified,
  );
  return accounts.length + addresses.length;
}

/**
 * The refusal for an address the person is about to add, or null when it may be added. Only a
 * proof on their own account refuses it, so the check never says whether an address is proven
 * on someone else's; the strip takes that one off once it is added.
 */
export async function addressRefusal(
  db: Kysely<Tables>,
  proofKey: Buffer | null,
  userId: string,
  email: string,
) {
  if (!domainOf(email)) return null;
  return workAddressRefusal(db, proofKey, email, new Date(), {
    ofUser: userId,
  });
}

/** What was taken off since the person last asked, marked told. */
export async function takeRemovals(
  db: Kysely<Tables>,
  userId: string,
): Promise<SignInRemoval[]> {
  const rows = await db
    .updateTable("sign_in_removals")
    .set({ told_at: new Date() })
    .where("user_id", "=", userId)
    .where("told_at", "is", null)
    .returning(["domain", "org_name", "provider", "removed_at"])
    .execute();
  return rows
    .sort((a, b) => a.removed_at.getTime() - b.removed_at.getTime())
    .map((row) => ({
      domain: row.domain,
      orgName: row.org_name,
      provider: row.provider,
      removedAt: row.removed_at.toISOString(),
    }));
}

export function signInRefusalWords(
  refusal: NonNullable<Awaited<ReturnType<typeof workAddressRefusal>>>,
): string {
  return refusal.why === "domain"
    ? `Addresses at ${refusal.domain} are work addresses, so they can't sign in to your account. Verify yours under Work addresses instead.`
    : "This address is already verified as a work address, so it can't sign in to your account.";
}
