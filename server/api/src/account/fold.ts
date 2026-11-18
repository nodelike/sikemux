import { createHash, randomBytes } from "node:crypto";
import type {
  FoldCatch,
  FoldIntent,
  FoldPreview,
  FoldWhy,
  OrgRole,
} from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";
import { sql, type Kysely, type Transaction } from "kysely";

import type { Services } from "../app.ts";
import { audit } from "../audit.ts";
import {
  isWeb,
  requireIdentity,
  requireRecentSignIn,
  type AuthEnv,
} from "../auth.ts";
import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import {
  ApiFailure,
  conflict,
  forbidden,
  notFound,
  readBody,
} from "../http.ts";
import { perUser } from "../limits.ts";
import { inviteTokenHash, seatsChanged } from "../orgs/shared.ts";
import { refusalOf, type Refusal } from "./admission.ts";
import type { ClerkBackend, SignUp } from "./clerk.ts";
import { deleteInClerk } from "./deletion.ts";
import { addressHash, domainOf } from "./proofs.ts";

const INTENT_MINUTES = 10;
/** How long after signing up an account is still asked whether the person already has one. */
const NEW_MINUTES = 30;

const ROLE_RANK: Record<OrgRole, number> = { user: 0, admin: 1, owner: 2 };

const NO_FOLD =
  "That link no longer works in this browser: it expired, was used, or was started in another one.";

const secretHash = (secret: string) =>
  createHash("sha256").update(secret).digest();

/**
 * What an account holds besides seats, by name. A fold only ever takes an account that holds
 * nothing else: anything here and both accounts stay as they are.
 */
export async function holdings(
  db: Kysely<Tables>,
  userId: string,
): Promise<string[]> {
  const { rows } = await sql<{ what: string }>`
    select what from (
      select 'a host or phone' as what where exists (select 1 from devices where user_id = ${userId})
        or exists (select 1 from removed_devices where user_id = ${userId})
      union all select 'a shared page' where exists (select 1 from shares where user_id = ${userId})
      union all select 'stashed items' where exists (select 1 from stash_items where user_id = ${userId})
      union all select 'webhooks' where exists (select 1 from hooks where user_id = ${userId})
      union all select 'a vault' where exists (select 1 from vaults where user_id = ${userId})
        or exists (select 1 from vault_uses where user_id = ${userId})
      union all select 'mods' where exists (select 1 from mod_counters where user_id = ${userId})
      union all select 'an org it made' where exists (select 1 from orgs where created_by = ${userId})
      union all select 'a purchase' where exists (select 1 from billing_customers where user_id = ${userId} or paid_by = ${userId})
        or exists (select 1 from billing_checkouts where opened_by = ${userId})
        or exists (select 1 from billing_people where user_id = ${userId})
        or exists (select 1 from plan_gifts where user_id = ${userId})
      union all select 'an org project share' where exists (
        select 1 from org_shares join org_users on org_users.id = org_shares.member_id
        where org_users.user_id = ${userId})
      union all select 'org vault entries' where exists (select 1 from org_vaults where created_by = ${userId})
        or exists (select 1 from org_vault_keys where wrapped_by = ${userId})
        or exists (select 1 from org_vault_entries where updated_by = ${userId})
    ) held`.execute(db);
  return rows.map((row) => row.what);
}

function holdsTooMuch(held: string[]): ApiFailure {
  return conflict(
    `That account holds ${held.join(", ")}, so it can't be brought over. Both accounts stay as they are.`,
  );
}

function requireWeb(c: Context<AuthEnv>) {
  if (!isWeb(c.get("identity")))
    throw forbidden("Accounts are brought together only on app.sikemux.com.");
}

async function inviteOrg(db: Kysely<Tables>, token: string | undefined) {
  if (!token) return null;
  return (
    (await db
      .selectFrom("org_invites")
      .innerJoin("orgs", "orgs.id", "org_invites.org_id")
      .select(["orgs.id", "orgs.name"])
      .where("token_hash", "=", inviteTokenHash(token))
      .where("accepted_at", "is", null)
      .where("revoked_at", "is", null)
      .where("expires_at", ">", sql<Date>`now()`)
      .executeTakeFirst()) ?? null
  );
}

async function orgName(db: Kysely<Tables>, orgId: string | null) {
  if (!orgId) return null;
  const org = await db
    .selectFrom("orgs")
    .select("name")
    .where("id", "=", orgId)
    .executeTakeFirst();
  return org?.name ?? null;
}

/** The org a proven address was proven for, by the account that holds the proof. */
async function proofOrg(db: Kysely<Tables>, key: Buffer, address: string) {
  const proof = await db
    .selectFrom("address_proofs")
    .leftJoin("orgs", "orgs.id", "address_proofs.org_id")
    .select(["orgs.id", "orgs.name"])
    .where("address_hash", "=", addressHash(key, address))
    .executeTakeFirst();
  return proof?.id && proof.name ? { id: proof.id, name: proof.name } : null;
}

async function signUpOf(clerk: ClerkBackend | null, userId: string) {
  const signUp = clerk ? await clerk.signUp(userId) : null;
  if (!signUp)
    throw new ApiFailure(
      503,
      "unavailable",
      "Sign-ins can't be read right now.",
    );
  return signUp;
}

function verifiedAddress(signUp: SignUp): string {
  if (!signUp.email || !domainOf(signUp.email))
    throw conflict(
      "This account's sign-in has no verified address to bring over.",
    );
  return signUp.email;
}

interface Side {
  from: string;
  into: string;
}

/**
 * Folds `from` into `into`, in one transaction that is safe to run twice: from's seats move to
 * into, keeping the higher role where both sit in one org; the work address's proof goes to into;
 * from is deleted, and Clerk is asked to delete its sign-in once this commits.
 */
async function fold(
  db: Kysely<Tables>,
  {
    foldId,
    secret,
    caller,
    address,
    proofKey,
  }: {
    foldId: string;
    secret: string;
    caller: string;
    /** The from side's verified address, read from Clerk before the transaction. */
    address: (side: Side) => Promise<string>;
    proofKey: Buffer;
  },
): Promise<Side> {
  const intent = await openIntent(db, foldId, secret, caller, {
    allowDone: true,
  });
  const side = sideOf(intent, caller);
  const sideAddress = intent.done_at ? null : await address(side);
  return db.transaction().execute(async (trx) => {
    const locked = await trx
      .selectFrom("account_folds")
      .selectAll()
      .where("id", "=", intent.id)
      .forUpdate()
      .executeTakeFirstOrThrow();
    if (locked.done_at) {
      if (locked.from_user === caller || locked.into_user === caller)
        return { from: locked.from_user ?? "", into: locked.into_user ?? "" };
      throw notFound(NO_FOLD);
    }
    if (locked.expires_at <= new Date()) throw notFound(NO_FOLD);
    await trx
      .insertInto("users")
      .values({ id: caller })
      .onConflict((oc) => oc.column("id").doNothing())
      .execute();
    const users = await trx
      .selectFrom("users")
      .select(["id", "deleted_at"])
      .where("id", "in", [side.from, side.into])
      .orderBy("id")
      .forUpdate()
      .execute();
    if (users.length !== 2 || users.some((user) => user.deleted_at))
      throw conflict(
        "One of the two accounts is deleted, so nothing was brought over.",
      );
    const held = await holdings(trx, side.from);
    if (held.length) throw holdsTooMuch(held);

    const email = sideAddress ?? "";
    const hash = locked.address_hash ?? addressHash(proofKey, email);
    if (sideAddress !== null && !hash.equals(addressHash(proofKey, email)))
      throw conflict(
        "The address on the account you just made changed since you started, so nothing was brought over. Start again from that account.",
      );
    const domain = locked.domain ?? domainOf(email) ?? "";
    const proven = await trx
      .selectFrom("address_proofs")
      .select("user_id")
      .where("address_hash", "=", hash)
      .executeTakeFirst();
    if (proven && proven.user_id !== side.from && proven.user_id !== side.into)
      throw conflict(
        "This address is verified on a third account, so nothing was brought over.",
      );

    const seats = await moveSeats(trx, side);
    const orgId = locked.org_id ?? (await seatOrg(trx, seats, domain));
    await trx
      .updateTable("address_proofs")
      .set({ user_id: side.into })
      .where("user_id", "=", side.from)
      .execute();
    await sql`
      insert into address_proofs (address_hash, user_id, domain, org_id, via)
      values (${hash}, ${side.into}, ${domain}, ${orgId}, 'fold')
      on conflict (address_hash) do update
        set user_id = excluded.user_id, domain = excluded.domain,
            org_id = coalesce(excluded.org_id, address_proofs.org_id), via = 'fold', proven_at = now()`.execute(
      trx,
    );

    await trx
      .updateTable("users")
      .set({
        deleted_at: sql<Date>`now()`,
        purge_after: sql<Date>`now()`,
        clerk_attempts: 0,
        clerk_retry_at: sql<Date>`now()`,
      })
      .where("id", "=", side.from)
      .execute();
    await appendEvent(trx, { userId: side.from, type: "account.deleted" });
    for (const userId of [side.from, side.into])
      await audit(trx, {
        userId,
        actor: `user:${caller}`,
        action: "account.folded",
        subject: locked.id,
        detail: {
          domain,
          why: locked.why,
          kept: userId === side.into,
          seats: seats.length,
        },
      });
    await trx
      .updateTable("account_folds")
      .set({
        from_user: side.from,
        into_user: side.into,
        address_hash: hash,
        domain,
        done_at: sql<Date>`now()`,
      })
      .where("id", "=", locked.id)
      .execute();
    return side;
  });
}

/**
 * The org a seat fold's proof is made for: the one that has verified the address's domain, else
 * the one whose seat was taken first.
 */
async function seatOrg(
  trx: Transaction<Tables>,
  seats: { org_id: string; joined_at: Date }[],
  domain: string,
): Promise<string | null> {
  if (seats.length === 0) return null;
  const verifying = await trx
    .selectFrom("org_domains")
    .select("org_id")
    .where("domain", "=", domain)
    .where("verified_at", "is not", null)
    .where(
      "org_id",
      "in",
      seats.map((seat) => seat.org_id),
    )
    .executeTakeFirst();
  if (verifying) return verifying.org_id;
  const [first] = [...seats].sort(
    (a, b) => a.joined_at.getTime() - b.joined_at.getTime(),
  );
  return first?.org_id ?? null;
}

async function moveSeats(trx: Transaction<Tables>, { from, into }: Side) {
  const seats = await trx
    .selectFrom("org_users")
    .select(["id", "org_id", "role", "joined_at"])
    .where("user_id", "=", from)
    .orderBy("org_id")
    .execute();
  for (const seat of seats) {
    await trx
      .selectFrom("orgs")
      .select("id")
      .where("id", "=", seat.org_id)
      .forUpdate()
      .execute();
    const kept = await trx
      .selectFrom("org_users")
      .select(["id", "role"])
      .where("org_id", "=", seat.org_id)
      .where("user_id", "=", into)
      .executeTakeFirst();
    if (!kept) {
      await trx
        .updateTable("org_users")
        .set({ user_id: into })
        .where("id", "=", seat.id)
        .execute();
      continue;
    }
    if (ROLE_RANK[seat.role] > ROLE_RANK[kept.role])
      await trx
        .updateTable("org_users")
        .set({ role: seat.role })
        .where("id", "=", kept.id)
        .execute();
    await sql`
      insert into org_team_members (team_id, member_id)
      select team_id, ${kept.id} from org_team_members where member_id = ${seat.id}
      on conflict do nothing`.execute(trx);
    await trx.deleteFrom("org_users").where("id", "=", seat.id).execute();
  }
  if (seats.length) await seatsChanged(trx, [from, into]);
  return seats;
}

type IntentRow = {
  id: string;
  from_user: string | null;
  into_user: string | null;
  why: FoldWhy;
  org_id: string | null;
  done_at: Date | null;
};

async function openIntent(
  db: Kysely<Tables>,
  foldId: string,
  secret: string,
  caller: string,
  { allowDone = false } = {},
): Promise<IntentRow> {
  if (!/^[0-9a-f-]{36}$/.test(foldId)) throw notFound(NO_FOLD);
  const intent = await db
    .selectFrom("account_folds")
    .select(["id", "from_user", "into_user", "why", "org_id", "done_at"])
    .where("id", "=", foldId)
    .where("secret_hash", "=", secretHash(secret))
    .where((eb) =>
      allowDone
        ? eb.or([
            eb("done_at", "is not", null),
            eb("expires_at", ">", sql<Date>`now()`),
          ])
        : eb.and([
            eb("done_at", "is", null),
            eb("expires_at", ">", sql<Date>`now()`),
          ]),
    )
    .executeTakeFirst();
  if (!intent) throw notFound(NO_FOLD);
  if (
    !intent.done_at &&
    (intent.from_user === caller || intent.into_user === caller)
  )
    throw new ApiFailure(
      409,
      "same_account",
      "This is the account you started from. Sign in with your other account to confirm it.",
    );
  return intent;
}

function sideOf(intent: IntentRow, caller: string): Side {
  return intent.from_user
    ? { from: intent.from_user, into: intent.into_user ?? caller }
    : { from: caller, into: intent.into_user ?? caller };
}

export function foldRoutes({
  database: { db },
  signInVerifier,
  limiter,
  clerk,
  addressProofKey: proofKey,
}: Pick<
  Services,
  "database" | "signInVerifier" | "limiter" | "clerk" | "addressProofKey"
>) {
  const refusal = async (userId: string, signUp: SignUp) => {
    const user = await db
      .selectFrom("users")
      .select("admitted_at")
      .where("id", "=", userId)
      .executeTakeFirst();
    return user?.admitted_at ? null : refusalOf(db, proofKey, userId, signUp);
  };

  const refusalOrg = async (found: Refusal, address: string) =>
    found.why === "domain"
      ? { id: found.orgId, name: found.orgName }
      : proofKey
        ? await proofOrg(db, proofKey, address)
        : null;

  const requireProofKey = () => {
    if (!proofKey)
      throw new ApiFailure(
        503,
        "unavailable",
        "Verifying work addresses is not set up on this server.",
      );
    return proofKey;
  };

  const requireAdmitted = async (userId: string) => {
    const user = await db
      .selectFrom("users")
      .select(["admitted_at", "deleted_at"])
      .where("id", "=", userId)
      .executeTakeFirst();
    if (!user?.admitted_at || user.deleted_at)
      throw forbidden(
        "The account that stays has to be one you already sign in to.",
      );
  };

  return new Hono<AuthEnv>()
    .use(
      "/fold/catch",
      requireIdentity(signInVerifier, db, { allowDeleted: true }),
    )
    .use("/fold/keep-new", requireIdentity(signInVerifier, db))
    .use("/folds", requireIdentity(signInVerifier, db))
    .use("/folds/*", requireIdentity(signInVerifier, db))
    .use("/fold/catch", perUser(limiter, "fold", 60))
    .use("/fold/keep-new", perUser(limiter, "fold", 60))
    .use("/folds", perUser(limiter, "fold", 60))
    .use("/folds/*", perUser(limiter, "fold", 60))
    .post("/fold/catch", async (c) => {
      requireWeb(c);
      const { userId } = c.get("identity");
      const { hint, inviteToken } = await readBody(c, "FoldCatchRequest");
      const quiet: FoldCatch = { ask: false, mustFold: false };
      const signUp = clerk ? await clerk.signUp(userId) : null;
      if (!signUp) return c.json(quiet);
      const deletion = await db
        .selectFrom("users")
        .select("purge_after")
        .where("id", "=", userId)
        .where("deleted_at", "is not", null)
        .executeTakeFirst();
      const found = await refusal(userId, signUp);
      if (found && signUp.email) {
        const org = await refusalOrg(found, signUp.email);
        const body: FoldCatch = {
          ask: true,
          mustFold: true,
          why: found.why,
          address: signUp.email,
          ...(org ? { orgName: org.name } : {}),
          ...(deletion?.purge_after
            ? { deletesAt: deletion.purge_after.toISOString() }
            : {}),
        };
        return c.json(body);
      }
      if (deletion || !hint || !signUp.email) return c.json(quiet);
      if (Date.now() - signUp.createdAt.getTime() > NEW_MINUTES * 60_000)
        return c.json(quiet);
      const user = await db
        .selectFrom("users")
        .select("new_kept_at")
        .where("id", "=", userId)
        .executeTakeFirst();
      if (user?.new_kept_at) return c.json(quiet);
      const seats = await db
        .selectFrom("org_users")
        .select("id")
        .where("user_id", "=", userId)
        .executeTakeFirst();
      if (seats || (await holdings(db, userId)).length) return c.json(quiet);
      const org = hint === "invite" ? await inviteOrg(db, inviteToken) : null;
      const body: FoldCatch = {
        ask: true,
        mustFold: false,
        why: hint,
        address: signUp.email,
        ...(org ? { orgName: org.name } : {}),
      };
      return c.json(body);
    })
    .post("/fold/keep-new", async (c) => {
      requireWeb(c);
      const { userId } = c.get("identity");
      const signUp = clerk ? await clerk.signUp(userId) : null;
      if (signUp && (await refusal(userId, signUp)))
        throw conflict(
          "This address can't have an account of its own. Sign in with your own Google, Apple or email, and bring the address there.",
        );
      await sql`
        insert into users (id, new_kept_at) values (${userId}, now())
        on conflict (id) do update set new_kept_at = coalesce(users.new_kept_at, now())`.execute(
        db,
      );
      return c.body(null, 204);
    })
    .post("/folds", requireRecentSignIn(), async (c) => {
      requireWeb(c);
      const key = requireProofKey();
      const { userId } = c.get("identity");
      const { why, inviteToken } = await readBody(c, "FoldStart");
      const secret = randomBytes(32).toString("base64url");
      const base = {
        secret_hash: secretHash(secret),
        why,
        expires_at: new Date(Date.now() + INTENT_MINUTES * 60_000),
      };
      let row;
      if (why === "seat") {
        await requireAdmitted(userId);
        row = await db
          .insertInto("account_folds")
          .values({ ...base, into_user: userId, org_id: null })
          .returning(["id", "expires_at"])
          .executeTakeFirstOrThrow();
      } else {
        const held = await holdings(db, userId);
        if (held.length) throw holdsTooMuch(held);
        const signUp = await signUpOf(clerk, userId);
        const address = verifiedAddress(signUp);
        const found = await refusal(userId, signUp);
        if ((why === "domain" || why === "address") && found?.why !== why)
          throw conflict("This account's address is not verified for an org.");
        const org = found
          ? await refusalOrg(found, address)
          : why === "invite"
            ? await inviteOrg(db, inviteToken)
            : null;
        await sql`insert into users (id) values (${userId}) on conflict do nothing`.execute(
          db,
        );
        row = await db
          .insertInto("account_folds")
          .values({
            ...base,
            from_user: userId,
            org_id: org?.id ?? null,
            address_hash: addressHash(key, address),
            domain: domainOf(address) ?? "",
          })
          .returning(["id", "expires_at"])
          .executeTakeFirstOrThrow();
      }
      c.get("log").info({ foldId: row.id, why }, "started a fold");
      const body: FoldIntent = {
        id: row.id,
        secret,
        expiresAt: row.expires_at.toISOString(),
      };
      return c.json(body, 201);
    })
    .post("/folds/:foldId/preview", async (c) => {
      requireWeb(c);
      const { userId } = c.get("identity");
      const { secret } = await readBody(c, "FoldSecret");
      const intent = await openIntent(
        db,
        c.req.param("foldId"),
        secret,
        userId,
      );
      const side = sideOf(intent, userId);
      if (side.into === intent.into_user && intent.from_user === null)
        await signUpOf(clerk, side.from).then(verifiedAddress);
      else await requireAdmitted(side.into);
      const held = await holdings(db, side.from);
      if (held.length) throw holdsTooMuch(held);
      const address = verifiedAddress(await signUpOf(clerk, side.from));
      const kept = clerk ? await clerk.profile(side.into) : null;
      const seats = await db
        .selectFrom("org_users")
        .innerJoin("orgs", "orgs.id", "org_users.org_id")
        .select("orgs.name")
        .where("org_users.user_id", "=", side.from)
        .orderBy("orgs.name")
        .execute();
      const body: FoldPreview = {
        why: intent.why,
        address,
        keptName: kept?.name ?? null,
        orgName: await orgName(db, intent.org_id),
        seats: seats.map((seat) => seat.name),
      };
      return c.json(body);
    })
    .post("/folds/:foldId/confirm", requireRecentSignIn(), async (c) => {
      requireWeb(c);
      const key = requireProofKey();
      const { userId } = c.get("identity");
      const { secret } = await readBody(c, "FoldSecret");
      const side = await fold(db, {
        foldId: c.req.param("foldId"),
        secret,
        caller: userId,
        proofKey: key,
        address: async (sides) => {
          if (sides.into === userId) await requireAdmitted(userId);
          return verifiedAddress(await signUpOf(clerk, sides.from));
        },
      });
      const log = c.get("log");
      log.info({ folded: side.from }, "folded an account");
      if (side.from) await deleteInClerk(db, clerk, log, side.from);
      return c.body(null, 204);
    });
}
