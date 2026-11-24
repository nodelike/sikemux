import { createHash } from "node:crypto";
import type { Org, OrgRole } from "@sikemux/protocol";
import type { Context } from "hono";
import type { Kysely, Selectable, Transaction } from "kysely";

import type { ClerkBackend, Person } from "../account/clerk.ts";
import { audit } from "../audit.ts";
import { holdStanding, type AuthEnv } from "../auth.ts";
import { checkSeatPauses } from "../billing/seat-pause.ts";
import { seatLimit, seatsHeld } from "../billing/seats.ts";
import { isFree } from "../billing/team.ts";
import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import { conflict, forbidden, notFound } from "../http.ts";

/** An invite link's token is kept only as this hash. */
export const inviteTokenHash = (token: string) =>
  createHash("sha256").update(token).digest("hex");

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const NOT_IN_ORG = "You are not in an org with that id.";

/** The id a request names in its path, or a 404 with `missing` when it cannot be one. */
export function idParam(c: Context, name: string, missing: string) {
  const id = c.req.param(name) ?? "";
  if (!UUID.test(id)) throw notFound(missing);
  return id;
}

export const isUuid = (value: string) => UUID.test(value);

export const orgIdOf = (c: Context) => idParam(c, "orgId", NOT_IN_ORG);

export async function orgBody(
  db: Kysely<Tables>,
  org: Selectable<Tables["orgs"]>,
  role: OrgRole,
): Promise<Org> {
  const [held, limit, preview] = await Promise.all([
    seatsHeld(db, org.id),
    seatLimit(db, org.id),
    isFree(db, org.id),
  ]);
  return {
    id: org.id,
    name: org.name,
    role,
    seats: { held, limit },
    ...(preview ? { preview: true } : {}),
    createdAt: org.created_at.toISOString(),
  };
}

/**
 * The org and the caller's role in it. Locking holds every other change to the org's members,
 * invites and seats until this transaction ends, so two changes never both take the last seat
 * or the last owner.
 */
export async function membership(
  db: Kysely<Tables>,
  orgId: string,
  userId: string,
  { lock = false } = {},
) {
  let query = db.selectFrom("orgs").selectAll().where("id", "=", orgId);
  if (lock) query = query.forUpdate();
  const org = await query.executeTakeFirst();
  const role = org
    ? (
        await db
          .selectFrom("org_users")
          .select("role")
          .where("org_id", "=", orgId)
          .where("user_id", "=", userId)
          .executeTakeFirst()
      )?.role
    : undefined;
  if (!org || !role) throw notFound(NOT_IN_ORG);
  return { org, role };
}

export function requireRole(role: OrgRole, allowed: readonly OrgRole[]) {
  if (!allowed.includes(role))
    throw forbidden(`Only an org's ${allowed.join(" or ")} may do this.`);
}

/** Refuses a change to the org's features while it has no live Team plan. */
export async function requireTeamPlan(db: Kysely<Tables>, orgId: string) {
  if ((await seatLimit(db, orgId)) === 0)
    throw conflict(
      "The org's Team plan lapsed, so its features are frozen until it is renewed.",
    );
}

/** The member a request names by its membership id. */
export async function memberOf(c: Context, db: Kysely<Tables>, orgId: string) {
  const memberId = idParam(
    c,
    "memberId",
    "Nobody in the org has that member id.",
  );
  const member = await db
    .selectFrom("org_users")
    .select(["id", "user_id", "role"])
    .where("org_id", "=", orgId)
    .where("id", "=", memberId)
    .executeTakeFirst();
  if (!member) throw notFound("Nobody in the org has that member id.");
  return member;
}

/** Tells `userIds`' hosts to read their orgs again. */
export async function orgsChanged(trx: Transaction<Tables>, userIds: string[]) {
  await appendEvents(
    trx,
    userIds.map((userId) => ({ userId, type: "orgs.changed" as const })),
  );
}

/**
 * A seat that starts or ends changes its holder's plan and orgs, so their apps are told, and
 * whether their own Pro is paused.
 */
export async function seatsChanged(
  trx: Transaction<Tables>,
  userIds: string[],
) {
  await checkSeatPauses(trx, userIds);
  await appendEvents(
    trx,
    userIds.flatMap((userId) => [
      { userId, type: "plan.changed" as const },
      { userId, type: "orgs.changed" as const },
    ]),
  );
}

/**
 * Puts `userId` in the org on a free seat. Refuses when they are in it already or no seat is
 * free, which is also so while the org has no live Team plan: joining never adds a seat.
 */
export async function admit(
  trx: Transaction<Tables>,
  {
    orgId,
    userId,
    role,
    invitedBy,
    detail,
  }: {
    orgId: string;
    userId: string;
    role: OrgRole;
    invitedBy: string | null;
    detail: Record<string, unknown>;
  },
) {
  await holdStanding(trx, userId);
  const org = await trx
    .selectFrom("orgs")
    .selectAll()
    .where("id", "=", orgId)
    .forUpdate()
    .executeTakeFirstOrThrow();
  if (
    await trx
      .selectFrom("org_users")
      .select("role")
      .where("org_id", "=", org.id)
      .where("user_id", "=", userId)
      .executeTakeFirst()
  )
    throw conflict("You are already in this org.");
  if ((await seatsHeld(trx, org.id)) >= (await seatLimit(trx, org.id)))
    throw conflict(
      "The org has no free seat: its Team plan lapsed or its seats were cut.",
    );
  await trx
    .insertInto("org_users")
    .values({ org_id: org.id, user_id: userId, role, invited_by: invitedBy })
    .execute();
  await audit(trx, {
    userId,
    actor: `user:${userId}`,
    action: "org.joined",
    subject: org.id,
    detail: { role, ...detail },
  });
  await seatsChanged(trx, [userId]);
  await orgsChanged(trx, [userId]);
  return org;
}

/** Names people by their sign-in, as far as Clerk answers; whoever it cannot name is left out. */
export function peopleNamer(clerk: ClerkBackend | null) {
  return async (
    c: Context<AuthEnv>,
    userIds: string[],
  ): Promise<Map<string, Person>> => {
    if (!clerk || userIds.length === 0) return new Map();
    const found = new Map<string, Person>();
    try {
      for (let at = 0; at < userIds.length; at += 100)
        for (const [id, person] of await clerk.people(
          userIds.slice(at, at + 100),
        ))
          found.set(id, person);
    } catch (error) {
      c.get("log").warn(
        { err: error },
        "Clerk could not name an org's members",
      );
    }
    return found;
  };
}
