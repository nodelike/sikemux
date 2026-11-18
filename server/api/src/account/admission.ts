import { sql, type Kysely } from "kysely";

import type { Verifier } from "../auth.ts";
import type { Tables } from "../db.ts";
import { ApiFailure } from "../http.ts";
import type { Logger } from "../log.ts";
import type { ClerkBackend, SignUp } from "./clerk.ts";
import { addressHash, domainOf } from "./proofs.ts";

/** Why a new account's sign-in address may not stand as an account of its own. */
export type Refusal =
  | { why: "domain"; domain: string; orgId: string; orgName: string }
  | { why: "address"; domain: string };

const REFUSAL_SECONDS = 60;

const refusingSince = new WeakMap<Kysely<Tables>, Promise<Date | null>>();

/** When the migration that began refusing sign-ups ran here: accounts made before it stand. */
function sinceRefusing(db: Kysely<Tables>): Promise<Date | null> {
  let since = refusingSince.get(db);
  if (!since) {
    since = sql<{ applied_at: Date }>`
      select applied_at from schema_migrations where name = 'one_account'`
      .execute(db)
      .then(({ rows }) => rows[0]?.applied_at ?? null);
    refusingSince.set(db, since);
  }
  return since;
}

/**
 * Whether an address added at `addedAt` came after an org verified its domain, or after the
 * address was proven on an account (other than `exceptUser`). Either way it is someone's work
 * address and opens no account.
 */
export async function workAddressRefusal(
  db: Kysely<Tables>,
  proofKey: Buffer | null,
  email: string,
  addedAt: Date,
  { exceptUser }: { exceptUser?: string } = {},
): Promise<Refusal | null> {
  const domain = domainOf(email);
  if (!domain) return null;
  const org = await db
    .selectFrom("org_domains")
    .innerJoin("orgs", "orgs.id", "org_domains.org_id")
    .select(["orgs.id", "orgs.name"])
    .where("org_domains.domain", "=", domain)
    .where("org_domains.verified_at", "<", addedAt)
    .executeTakeFirst();
  if (org) return { why: "domain", domain, orgId: org.id, orgName: org.name };
  if (!proofKey) return null;
  let proofs = db
    .selectFrom("address_proofs")
    .select("user_id")
    .where("address_hash", "=", addressHash(proofKey, email))
    .where("proven_at", "<", addedAt);
  if (exceptUser) proofs = proofs.where("user_id", "!=", exceptUser);
  return (await proofs.executeTakeFirst()) ? { why: "address", domain } : null;
}

/**
 * Whether a sign-up's address may not stand as an account of its own. A sign-in an account
 * already had is never taken away by a proof made later, and an account made before this server
 * began refusing stands as it is.
 */
export async function refusalOf(
  db: Kysely<Tables>,
  proofKey: Buffer | null,
  userId: string,
  signUp: SignUp,
): Promise<Refusal | null> {
  if (!signUp.email) return null;
  const since = await sinceRefusing(db);
  if (!since || signUp.createdAt < since) return null;
  return workAddressRefusal(db, proofKey, signUp.email, signUp.createdAt, {
    exceptUser: userId,
  });
}

export function refusalWords(refusal: Refusal): string {
  return refusal.why === "domain"
    ? `${refusal.orgName} has verified ${refusal.domain}, so an address there can't have a Sikemux account of its own. Sign in with your own Google, Apple or email instead, then bring the address to that account on app.sikemux.com.`
    : `This address is already verified on a Sikemux account, so it can't have one of its own. Sign in to that account with its own Google, Apple or email instead.`;
}

/** Records that an account may stand, making its users row if no route has yet. */
export async function markAdmitted(db: Kysely<Tables>, userId: string) {
  await sql`
    insert into users (id, admitted_at) values (${userId}, now())
    on conflict (id) do update set admitted_at = coalesce(users.admitted_at, now())`.execute(
    db,
  );
}

/**
 * The sign-up check every Clerk sign-in passes once. A host's or a phone's signed request is
 * already on a standing account, so only bearer tokens come through here.
 */
export function admittingVerifier(
  verifier: Verifier,
  db: Kysely<Tables>,
  clerk: ClerkBackend | null,
  proofKey: Buffer | null,
  log: Logger,
): Verifier {
  const admitted = new Set<string>();
  const refused = new Map<string, { until: number; words: string }>();
  if (!clerk)
    log.warn(
      "CLERK_SECRET_KEY is not set: every new account is admitted without checking its address",
    );

  const check = async (userId: string) => {
    const row = await db
      .selectFrom("users")
      .select("admitted_at")
      .where("id", "=", userId)
      .executeTakeFirst();
    if (row?.admitted_at) return;
    const signUp = clerk ? await clerk.signUp(userId) : null;
    if (clerk && !signUp) return;
    const refusal = signUp
      ? await refusalOf(db, proofKey, userId, signUp)
      : null;
    if (refusal) {
      const words = refusalWords(refusal);
      refused.set(userId, {
        until: Date.now() + REFUSAL_SECONDS * 1000,
        words,
      });
      throw new ApiFailure(403, "address_proven", words);
    }
    await markAdmitted(db, userId);
  };

  return {
    async verify(token) {
      const identity = await verifier.verify(token);
      if (admitted.has(identity.userId)) return identity;
      const held = refused.get(identity.userId);
      if (held && held.until > Date.now())
        throw new ApiFailure(403, "address_proven", held.words);
      refused.delete(identity.userId);
      await check(identity.userId);
      admitted.add(identity.userId);
      return identity;
    },
  };
}
