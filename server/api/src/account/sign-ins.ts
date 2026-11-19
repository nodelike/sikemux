import type { SignInRemoval } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";
import {
  goneAddressRefusal,
  workAddressRefusal,
  type Refusal,
} from "./admission.ts";
import { CLERK_CLOCK_SLACK_MS } from "../auth.ts";
import { removeDevice } from "../devices/removal.ts";
import { alertStaff } from "../sentry.ts";
import {
  ClerkRefusal,
  type ClerkBackend,
  type ClerkSession,
  type SignIns,
} from "./clerk.ts";
import { addressHash, domainOf } from "./proofs.ts";

const FROZEN =
  "Froze an account: a session through a work address may have taken it over";
const UNDATED = "Left kinds of sign-in the end of a work sign-in can't date";
const STRANDED =
  "Left a work address on an account: taking it off would leave no way in";
const LIFT =
  "In the API's database: update users set frozen_at = null where encode(sha256(convert_to('sikemux-usage|' || id, 'UTF8')), 'hex') = '<the issue's user id>'";
const BEFORE_LIFTING =
  "Remove every way listed under added from the account in Clerk's dashboard first, or lifting the freeze hands the account back. Ways the person lost in that time are not restored.";

/**
 * Takes off the account every address added after it was admitted that is a work address, with
 * the connected account that brought it, so the work Google stops signing in too. Then ends every
 * session made since it first appeared, with what those sessions put on the account and every
 * other way to sign in added since. Nothing the account was admitted with goes. When the work
 * address can't go because the person's own ways are gone, the account is frozen instead.
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
  if (!user) return;
  const signIns = await clerk.signIns(userId);
  if (!signIns) return;
  const seen = await noteAddresses(db, proofKey, userId, [
    ...signIns.addresses,
    ...signIns.externalAccounts
      .filter((account) => account.email)
      .map((account) => ({ ...account, signsIn: true })),
  ]);
  const admittedAt = user.admitted_at;
  if (!admittedAt) return;

  const addressed = new Set(
    signIns.addresses.map((address) => normalAddress(address.email)),
  );
  const workSignIns = [
    ...signIns.addresses.map((address) => ({
      address,
      email: address.email,
      addedAt: address.addedAt,
      brought: signIns.externalAccounts.filter(
        (account) =>
          normalAddress(account.email) === normalAddress(address.email),
      ),
    })),
    ...signIns.externalAccounts
      .filter((account) => !addressed.has(normalAddress(account.email)))
      .map((account) => ({
        address: null,
        email: account.email,
        addedAt: account.addedAt,
        brought: [account],
      })),
  ];

  for (const { address, email, addedAt, brought } of workSignIns) {
    if (addedAt <= admittedAt) continue;
    const refusal = await workAddressRefusal(db, proofKey, email, addedAt);
    if (!refusal) continue;
    if (address && !address.signsIn && brought.length === 0) continue;
    const stretch = seen.get(normalAddress(email));
    const appeared = new Date(
      Math.min(
        (stretch?.firstSeen ?? addedAt).getTime(),
        ...brought.map((account) => account.addedAt.getTime()),
      ),
    );
    const primary =
      address && signIns.primaryAddressId === address.id
        ? signIns.addresses.find(
            (other) =>
              other.id !== address.id &&
              other.verified &&
              other.addedAt <= admittedAt,
          )
        : null;
    const stuck =
      waysLeft(signIns, address?.id ?? null, brought) === 0
        ? "the work address is the last way in"
        : primary === undefined
          ? "the work address is primary and no earlier address is verified"
          : null;
    if (stuck) {
      await freezeIfSignedInSince(db, clerk, log, userId, {
        signIns,
        since: appeared,
        admittedAt,
        why: stuck,
      });
      continue;
    }
    await recordRemoval(db, userId, {
      clerkId: address?.id ?? brought[0]?.id ?? "",
      refusal,
      provider: brought[0]?.provider ?? null,
      appeared,
      stretch: stretch?.id ?? null,
    });
    if (primary) {
      await clerk.setPrimaryAddress(userId, primary.id);
      signIns.primaryAddressId = primary.id;
    }
    for (const account of brought)
      await clerk.removeExternalAccount(userId, account.id);
    if (address) await clerk.removeAddress(address.id);
    signIns.addresses = signIns.addresses.filter((a) => a !== address);
    signIns.externalAccounts = signIns.externalAccounts.filter(
      (account) => !brought.includes(account),
    );
    log.info(
      { userId, domain: refusal.domain },
      "took a work address off a sign-in",
    );
  }
  if (proofKey)
    await endGoneStretches(db, proofKey, log, userId, {
      present: [...seen.values()].map((stretch) => stretch.id),
      admittedAt,
    });
  await endSessionsOfRemoved(db, clerk, proofKey, log, userId, admittedAt);
}

/**
 * Records that a work sign-in was taken off, so the sessions made since it appeared are ended,
 * and ends its stretch. `clerkId` is the Clerk address, or connected account, that held it.
 */
async function recordRemoval(
  db: Kysely<Tables>,
  userId: string,
  {
    clerkId,
    refusal,
    provider,
    appeared,
    stretch,
  }: {
    clerkId: string;
    refusal: Refusal;
    provider: string | null;
    appeared: Date;
    stretch: string | null;
  },
) {
  const { domain } = refusal;
  await db.transaction().execute(async (trx) => {
    if (stretch)
      await trx
        .updateTable("addresses_seen")
        .set({ ended_at: new Date() })
        .where("id", "=", stretch)
        .where("ended_at", "is", null)
        .execute();
    const recorded = await trx
      .insertInto("sign_in_removals")
      .values({
        user_id: userId,
        address_id: clerkId,
        domain,
        org_name: refusal.why === "domain" ? refusal.orgName : null,
        provider,
        end_sessions_from: appeared,
        address_seen_id: stretch,
      })
      .onConflict((oc) => oc.columns(["user_id", "address_id"]).doNothing())
      .returning("id")
      .executeTakeFirst();
    if (recorded)
      await audit(trx, {
        userId,
        actor: "system",
        action: "account.work_sign_in_removed",
        detail: { domain, provider },
      });
  });
}

/**
 * Ends the stretches of addresses no longer on the account. One that could sign in and was a work
 * address when it appeared was deleted before the strip could take it, so it counts as taken off
 * then, and the sessions made since it appeared are ended like any other's.
 */
async function endGoneStretches(
  db: Kysely<Tables>,
  proofKey: Buffer,
  log: Logger,
  userId: string,
  { present, admittedAt }: { present: string[]; admittedAt: Date },
) {
  let gone = db
    .selectFrom("addresses_seen")
    .select([
      "id",
      "address_hash",
      "domain_hash",
      "first_seen",
      "could_sign_in",
    ])
    .where("user_id", "=", userId)
    .where("ended_at", "is", null);
  if (present.length) gone = gone.where("id", "not in", present);
  for (const stretch of await gone.execute()) {
    const refusal =
      stretch.could_sign_in && stretch.first_seen > admittedAt
        ? await goneAddressRefusal(db, proofKey, stretch, stretch.first_seen)
        : null;
    if (!refusal) {
      await db
        .updateTable("addresses_seen")
        .set({ ended_at: new Date() })
        .where("id", "=", stretch.id)
        .execute();
      continue;
    }
    await recordRemoval(db, userId, {
      clerkId: `deleted_${stretch.id}`,
      refusal,
      provider: null,
      appeared: stretch.first_seen,
      stretch: stretch.id,
    });
    log.warn(
      { userId, domain: refusal.domain },
      "a work address was deleted before the strip could take it",
    );
  }
}

const normalAddress = (email: string) => email.trim().toLowerCase();

/**
 * Records each address in the stretch of time it has been on the account, and returns that
 * stretch by address. A stretch starts at the earliest sighting and ends when the strip takes the
 * address away; Clerk's own time starts again when an address is added back, so a sighting older
 * than a stretch's end widens that stretch, and its removal is swept again from the new start.
 */
export async function noteAddresses(
  db: Kysely<Tables>,
  proofKey: Buffer | null,
  userId: string,
  addresses: { email: string; addedAt: Date; signsIn: boolean }[],
): Promise<Map<string, { id: string; firstSeen: Date }>> {
  const seen = new Map<string, { id: string; firstSeen: Date }>();
  if (!proofKey) return seen;
  const user = await db
    .selectFrom("users")
    .select("id")
    .where("id", "=", userId)
    .executeTakeFirst();
  if (!user) return seen;
  const earliestByAddress = new Map<string, { at: Date; signsIn: boolean }>();
  for (const { email, addedAt, signsIn } of addresses) {
    const known = earliestByAddress.get(normalAddress(email));
    earliestByAddress.set(normalAddress(email), {
      at: known && known.at < addedAt ? known.at : addedAt,
      signsIn: signsIn || (known?.signsIn ?? false),
    });
  }
  for (const [email, { at, signsIn }] of earliestByAddress) {
    const hash = addressHash(proofKey, email);
    const ended = await db
      .selectFrom("addresses_seen")
      .select("id")
      .where("user_id", "=", userId)
      .where("address_hash", "=", hash)
      .where("ended_at", ">", at)
      .orderBy("ended_at")
      .limit(1)
      .executeTakeFirst();
    const row = ended
      ? await db
          .updateTable("addresses_seen")
          .set({
            first_seen: sql<Date>`least(first_seen, ${at})`,
            could_sign_in: sql<boolean>`could_sign_in or ${signsIn}`,
          })
          .where("id", "=", ended.id)
          .returning(["id", "first_seen"])
          .executeTakeFirstOrThrow()
      : await db
          .insertInto("addresses_seen")
          .values({
            user_id: userId,
            address_hash: hash,
            domain_hash: addressHash(proofKey, domainOf(email) ?? ""),
            first_seen: at,
            could_sign_in: signsIn,
          })
          .onConflict((oc) =>
            oc
              .columns(["user_id", "address_hash"])
              .where("ended_at", "is", null)
              .doUpdateSet({
                first_seen: sql<Date>`least(addresses_seen.first_seen, excluded.first_seen)`,
                could_sign_in: sql<boolean>`addresses_seen.could_sign_in or excluded.could_sign_in`,
              }),
          )
          .returning(["id", "first_seen"])
          .executeTakeFirstOrThrow();
    seen.set(email, { id: row.id, firstSeen: row.first_seen });
  }
  await sql`
    update sign_in_removals removal
    set end_sessions_from = stretch.first_seen
    from addresses_seen stretch
    where removal.user_id = ${userId}
      and removal.address_seen_id = stretch.id
      and stretch.first_seen < coalesce(removal.end_sessions_from, removal.swept_from)
  `.execute(db);
  return seen;
}

/**
 * Ends every session made since a removed address first appeared, refuses their tokens from now
 * on, and takes off the account what they put on it and every way to sign in added since. A
 * removal counts only once a fresh read shows its address gone; one whose address the org no
 * longer counts as theirs is dropped.
 */
async function endSessionsOfRemoved(
  db: Kysely<Tables>,
  clerk: ClerkBackend,
  proofKey: Buffer | null,
  log: Logger,
  userId: string,
  admittedAt: Date,
) {
  const waiting = await db
    .selectFrom("sign_in_removals")
    .select(["id", "address_id", "end_sessions_from"])
    .where("user_id", "=", userId)
    .where("end_sessions_from", "is not", null)
    .execute();
  if (waiting.length === 0) return;
  const fresh = await clerk.signIns(userId);
  if (!fresh) return;
  const gone: typeof waiting = [];
  for (const row of waiting) {
    const address = fresh.addresses.find((a) => a.id === row.address_id);
    if (!address) gone.push(row);
    else if (
      !(await workAddressRefusal(db, proofKey, address.email, address.addedAt))
    ) {
      await db
        .deleteFrom("sign_in_removals")
        .where("id", "=", row.id)
        .execute();
      log.info({ userId }, "dropped a removal its org no longer asks for");
    }
  }
  if (gone.length === 0) return;

  const from = Math.min(
    ...gone.map((row) => row.end_sessions_from?.getTime() ?? Infinity),
  );
  const opened = (await clerk.sessions(userId)).filter(
    (session) => session.createdAt.getTime() >= from,
  );
  let devices = 0;
  if (opened.length > 0) {
    devices = await endSessions(db, clerk, userId, opened);
    await takeWaysAddedSince(db, clerk, log, userId, fresh, {
      since: new Date(from),
      admittedAt,
    });
  }
  await db
    .updateTable("sign_in_removals")
    .set({
      swept_from: sql<Date>`least(swept_from, end_sessions_from)`,
      end_sessions_from: null,
    })
    .where(
      "id",
      "in",
      gone.map((row) => row.id),
    )
    .execute();
  if (opened.length > 0)
    log.info(
      { userId, sessions: opened.length, devices },
      "ended the sessions a removed work address could have opened",
    );
}

/** Ends the sessions, refuses their tokens, and removes the devices and open folds they made. */
async function endSessions(
  db: Kysely<Tables>,
  clerk: ClerkBackend,
  userId: string,
  opened: ClerkSession[],
): Promise<number> {
  const firstMade = Math.min(...opened.map((s) => s.createdAt.getTime()));
  const ids = opened.map((session) => session.id);
  const devices = await db.transaction().execute(async (trx) => {
    await trx
      .selectFrom("users")
      .select("id")
      .where("id", "=", userId)
      .forUpdate()
      .execute();
    await trx
      .insertInto("ended_sessions")
      .values(
        opened.map((session) => ({
          session_id: session.id,
          user_id: userId,
          made_at: session.createdAt,
        })),
      )
      .onConflict((oc) => oc.column("session_id").doNothing())
      .execute();
    const added = await trx
      .selectFrom("devices")
      .select("key")
      .where("user_id", "=", userId)
      .where((eb) =>
        eb.or([
          eb("sign_in_session", "in", ids),
          eb(
            "mac_signed_in_at",
            ">=",
            new Date(firstMade - CLERK_CLOCK_SLACK_MS),
          ),
        ]),
      )
      .execute();
    for (const { key } of added)
      await removeDevice(trx, {
        userId,
        key,
        reason: "removed",
        actor: "system",
        detail: { why: "work_sign_in_removed" },
      });
    await trx
      .updateTable("account_folds")
      .set({ expires_at: new Date() })
      .where("started_session", "in", ids)
      .where("done_at", "is", null)
      .where("expires_at", ">", new Date())
      .execute();
    return added.length;
  });
  for (const session of opened)
    if (session.live) await clerk.revokeSession(session.id);
  return devices;
}

type Window = { since: Date; admittedAt: Date };

/** The ways of the kinds Clerk dates added since `since`; none the account was admitted with. */
function addedIn(signIns: SignIns, { since, admittedAt }: Window) {
  const added = (at: Date | null) =>
    at !== null && at >= since && at > admittedAt;
  return {
    addresses: signIns.addresses.filter((a) => added(a.addedAt)),
    accounts: signIns.externalAccounts.filter((a) => added(a.addedAt)),
    phones: signIns.phones.filter((p) => added(p.addedAt)),
    wallets: signIns.wallets.filter((w) => added(w.addedAt)),
    password: added(signIns.passwordSetAt),
  };
}

/** What staff take off by hand before the freeze is lifted: kinds and Clerk ids, never an address. */
function listed(ways: ReturnType<typeof addedIn>) {
  return [
    ...ways.addresses.map((a) => ({ kind: "email address", id: a.id })),
    ...ways.accounts.map((a) => ({ kind: `${a.provider} account`, id: a.id })),
    ...ways.phones.map((p) => ({ kind: "phone", id: p.id })),
    ...ways.wallets.map((w) => ({ kind: "wallet", id: w.id })),
    ...(ways.password ? [{ kind: "password", id: null }] : []),
  ];
}

/**
 * Takes off the ways to sign in added in the window, of the kinds Clerk dates; a password set in it
 * is replaced, since Clerk can't remove one. When that would leave no older way in, or the password
 * can't be replaced, someone took the account over in the window, so it is frozen instead.
 */
async function takeWaysAddedSince(
  db: Kysely<Tables>,
  clerk: ClerkBackend,
  log: Logger,
  userId: string,
  signIns: SignIns,
  window: Window,
) {
  if (signIns.undated.length) {
    log.warn(
      { userId, kinds: signIns.undated },
      "left kinds of sign-in the strip can't date",
    );
    alertStaff(userId, UNDATED, { kinds: signIns.undated });
  }
  const ways = addedIn(signIns, window);
  const { addresses, accounts, phones, wallets, password } = ways;
  const taking =
    addresses.length + accounts.length + phones.length + wallets.length;
  if (taking === 0 && !password) return;

  const olderAddresses = signIns.addresses.filter(
    (a) => !addresses.includes(a) && a.signsIn,
  );
  const older =
    olderAddresses.length +
    signIns.externalAccounts.length -
    accounts.length +
    signIns.phones.filter((p) => !phones.includes(p) && p.signsIn).length +
    signIns.wallets.length -
    wallets.length;
  const primary = addresses.some((a) => a.id === signIns.primaryAddressId)
    ? olderAddresses.find((a) => a.verified)
    : null;
  if (older === 0 || primary === undefined) {
    await freeze(db, log, userId, "nothing older would be left", ways);
    return;
  }
  if (primary) await clerk.setPrimaryAddress(userId, primary.id);
  for (const account of accounts)
    await clerk.removeExternalAccount(userId, account.id);
  for (const address of addresses) await clerk.removeAddress(address.id);
  for (const phone of phones) await clerk.removePhone(phone.id);
  for (const wallet of wallets) await clerk.removeWallet(userId, wallet.id);
  if (password && !(await replacedPassword(clerk, userId, signIns))) {
    await freeze(db, log, userId, "the password set then can't be replaced", {
      addresses: [],
      accounts: [],
      phones: [],
      wallets: [],
      password: true,
    });
    return;
  }
  log.info(
    { userId, taken: taking, password },
    "took off the ways to sign in added since a work address appeared",
  );
}

/** Freezes the account when a session was made since `since`, after ending those sessions. */
async function freezeIfSignedInSince(
  db: Kysely<Tables>,
  clerk: ClerkBackend,
  log: Logger,
  userId: string,
  { signIns, why, ...window }: Window & { signIns: SignIns; why: string },
) {
  const opened = (await clerk.sessions(userId)).filter(
    (session) => session.createdAt >= window.since,
  );
  if (opened.length === 0) {
    log.warn({ userId }, `left a work address: ${why}`);
    alertStaff(userId, STRANDED, { why });
    return;
  }
  await endSessions(db, clerk, userId, opened);
  await freeze(db, log, userId, why, addedIn(signIns, window));
}

/** Whether a fresh read shows the password changed since `signIns`, after asking Clerk to replace it. */
async function replacedPassword(
  clerk: ClerkBackend,
  userId: string,
  signIns: SignIns,
) {
  try {
    await clerk.replacePassword(userId);
  } catch (error) {
    if (!(error instanceof ClerkRefusal)) throw error;
    return false;
  }
  const before = signIns.passwordSetAt?.getTime() ?? 0;
  const after = (await clerk.signIns(userId))?.passwordSetAt?.getTime() ?? 0;
  return after > before;
}

/**
 * Marks the account frozen, so no sign-in session or Mac sign-in is taken and no device is added
 * until staff lift it, and removes nothing more. The staff issue says what to remove first.
 */
async function freeze(
  db: Kysely<Tables>,
  log: Logger,
  userId: string,
  why: string,
  ways: ReturnType<typeof addedIn>,
) {
  alertStaff(userId, FROZEN, {
    why,
    added: listed(ways),
    beforeLifting: BEFORE_LIFTING,
    toLift: LIFT,
  });
  await db
    .updateTable("users")
    .set({ frozen_at: sql<Date>`coalesce(frozen_at, now())` })
    .where("id", "=", userId)
    .execute();
  log.warn({ userId, why }, "froze an account a work sign-in may have taken");
}
function waysLeft(
  signIns: SignIns,
  addressId: string | null,
  brought: SignIns["externalAccounts"],
) {
  const accounts = signIns.externalAccounts.filter((a) => !brought.includes(a));
  const addresses = signIns.addresses.filter(
    (a) => a.id !== addressId && a.signsIn,
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
