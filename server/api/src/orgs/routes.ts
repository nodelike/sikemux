import { createHash, randomBytes } from "node:crypto";
import type {
  Org,
  OrgInvite,
  OrgInviteCreated,
  OrgInviteList,
  OrgList,
  OrgMember,
  OrgMemberList,
  OrgRole,
} from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";
import { sql, type Kysely, type Selectable, type Transaction } from "kysely";

import type { Services } from "../app.ts";
import { audit } from "../audit.ts";
import type { Person } from "../account/clerk.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import { requireBillingAccess } from "../billing/access.ts";
import { hasOpenSubscriptions, seatLimit } from "../billing/seats.ts";
import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import {
  badRequest,
  conflict,
  forbidden,
  notFound,
  readBody,
} from "../http.ts";
import { perUser } from "../limits.ts";
import { manages } from "./roles.ts";

const INVITE_DAYS = 7;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const NOT_IN_ORG = "You are not in an org with that id.";

const tokenHash = (token: string) =>
  createHash("sha256").update(token).digest("hex");

const openInvites = (db: Kysely<Tables>, orgId: string) =>
  db
    .selectFrom("org_invites")
    .where("org_id", "=", orgId)
    .where("accepted_at", "is", null)
    .where("revoked_at", "is", null)
    .where("expires_at", ">", sql<Date>`now()`);

async function inviteCount(db: Kysely<Tables>, orgId: string) {
  const { count } = await openInvites(db, orgId)
    .select((eb) => eb.fn.countAll<string>().as("count"))
    .executeTakeFirstOrThrow();
  return Number(count);
}

async function memberCount(db: Kysely<Tables>, orgId: string, role?: OrgRole) {
  let query = db.selectFrom("org_users").where("org_id", "=", orgId);
  if (role) query = query.where("role", "=", role);
  const { count } = await query
    .select((eb) => eb.fn.countAll<string>().as("count"))
    .executeTakeFirstOrThrow();
  return Number(count);
}

/** Members and pending invites each hold a seat. */
async function seatsHeld(db: Kysely<Tables>, orgId: string) {
  return (await memberCount(db, orgId)) + (await inviteCount(db, orgId));
}

function orgName(name: string) {
  const trimmed = name.trim();
  if (!trimmed) throw badRequest("An org's name cannot be blank.");
  return trimmed;
}

async function orgBody(
  db: Kysely<Tables>,
  org: Selectable<Tables["orgs"]>,
  role: OrgRole,
): Promise<Org> {
  const [held, limit] = await Promise.all([
    seatsHeld(db, org.id),
    seatLimit(db, org.id),
  ]);
  return {
    id: org.id,
    name: org.name,
    role,
    seats: { held, limit },
    createdAt: org.created_at.toISOString(),
  };
}

function inviteBody(invite: Selectable<Tables["org_invites"]>): OrgInvite {
  return {
    id: invite.id,
    role: invite.role,
    invitedBy: invite.invited_by,
    createdAt: invite.created_at.toISOString(),
    expiresAt: invite.expires_at.toISOString(),
  };
}

function orgIdOf(c: Context) {
  const orgId = c.req.param("orgId") ?? "";
  if (!UUID.test(orgId)) throw notFound(NOT_IN_ORG);
  return orgId;
}

/**
 * The org and the caller's role in it. Locking holds every other change to the org's members,
 * invites and seats until this transaction ends, so two changes never both take the last seat
 * or the last owner.
 */
async function membership(
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

function requireRole(role: OrgRole, allowed: readonly OrgRole[]) {
  if (!allowed.includes(role))
    throw forbidden(`Only an org's ${allowed.join(" or ")} may do this.`);
}

async function memberRole(db: Kysely<Tables>, orgId: string, userId: string) {
  const member = await db
    .selectFrom("org_users")
    .select("role")
    .where("org_id", "=", orgId)
    .where("user_id", "=", userId)
    .executeTakeFirst();
  if (!member) throw notFound("Nobody in the org has that user id.");
  return member.role;
}

/** Refuses taking `userId`, who holds `role`, out of the owners when they are the last one. */
async function keepAnOwner(
  db: Kysely<Tables>,
  orgId: string,
  role: OrgRole,
  message: string,
) {
  if (role === "owner" && (await memberCount(db, orgId, "owner")) <= 1)
    throw conflict(message);
}

/** A seat that starts or ends changes its holder's plan, so their apps are told. */
async function seatsChanged(trx: Transaction<Tables>, userIds: string[]) {
  await appendEvents(
    trx,
    userIds.map((userId) => ({ userId, type: "plan.changed" as const })),
  );
}

export function orgRoutes({
  database: { db },
  verifier,
  limiter,
  clerk,
  appOrigin,
  billing: { live },
}: Pick<
  Services,
  "database" | "verifier" | "limiter" | "clerk" | "appOrigin" | "billing"
>) {
  const people = async (
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

  return new Hono<AuthEnv>()
    .use(requireIdentity(verifier, db))
    .use(requireBillingAccess(db, live))
    .post("/", perUser(limiter, "create-org", 10), async (c) => {
      const { userId } = c.get("identity");
      const { name } = await readBody(c, "OrgCreate");
      const org = await db.transaction().execute(async (trx) => {
        await trx
          .insertInto("users")
          .values({ id: userId })
          .onConflict((oc) => oc.column("id").doNothing())
          .execute();
        const created = await trx
          .insertInto("orgs")
          .values({ name: orgName(name), created_by: userId })
          .returningAll()
          .executeTakeFirstOrThrow();
        await trx
          .insertInto("org_users")
          .values({ org_id: created.id, user_id: userId, role: "owner" })
          .execute();
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "org.created",
          subject: created.id,
        });
        return created;
      });
      c.get("log").info({ orgId: org.id }, "created an org");
      return c.json(await orgBody(db, org, "owner"), 201);
    })
    .get("/", async (c) => {
      const { userId } = c.get("identity");
      const rows = await db
        .selectFrom("orgs")
        .innerJoin("org_users", "org_users.org_id", "orgs.id")
        .selectAll("orgs")
        .select("org_users.role")
        .where("org_users.user_id", "=", userId)
        .orderBy("orgs.created_at")
        .orderBy("orgs.id")
        .execute();
      const body: OrgList = {
        orgs: await Promise.all(
          rows.map(({ role, ...org }) => orgBody(db, org, role)),
        ),
      };
      return c.json(body);
    })
    .post("/join", perUser(limiter, "join-org", 10), async (c) => {
      const { userId } = c.get("identity");
      const { token } = await readBody(c, "OrgJoin");
      const joined = await db.transaction().execute(async (trx) => {
        const invite = await trx
          .selectFrom("org_invites")
          .selectAll()
          .where("token_hash", "=", tokenHash(token))
          .where("accepted_at", "is", null)
          .where("revoked_at", "is", null)
          .where("expires_at", ">", sql<Date>`now()`)
          .executeTakeFirst();
        if (!invite)
          throw notFound(
            "No invite has that link, or it was used, revoked or has expired.",
          );
        const org = await trx
          .selectFrom("orgs")
          .selectAll()
          .where("id", "=", invite.org_id)
          .forUpdate()
          .executeTakeFirstOrThrow();
        const used = await trx
          .updateTable("org_invites")
          .set({ accepted_by: userId, accepted_at: sql<Date>`now()` })
          .where("id", "=", invite.id)
          .where("accepted_at", "is", null)
          .where("revoked_at", "is", null)
          .executeTakeFirst();
        if (used.numUpdatedRows === 0n)
          throw notFound("That invite was just used or revoked.");
        if (
          await trx
            .selectFrom("org_users")
            .select("role")
            .where("org_id", "=", org.id)
            .where("user_id", "=", userId)
            .executeTakeFirst()
        )
          throw conflict("You are already in this org.");
        if ((await memberCount(trx, org.id)) >= (await seatLimit(trx, org.id)))
          throw conflict(
            "The org has no free seat: its Team plan lapsed or its seats were cut.",
          );
        await trx
          .insertInto("users")
          .values({ id: userId })
          .onConflict((oc) => oc.column("id").doNothing())
          .execute();
        await trx
          .insertInto("org_users")
          .values({
            org_id: org.id,
            user_id: userId,
            role: invite.role,
            invited_by: invite.invited_by,
          })
          .execute();
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "org.joined",
          subject: org.id,
          detail: { role: invite.role, inviteId: invite.id },
        });
        await seatsChanged(trx, [userId]);
        return { org, role: invite.role };
      });
      c.get("log").info({ orgId: joined.org.id }, "joined an org");
      return c.json(await orgBody(db, joined.org, joined.role));
    })
    .post("/:orgId/name", perUser(limiter, "rename-org", 30), async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      const { name } = await readBody(c, "OrgRename");
      const { org, role } = await membership(db, orgId, userId);
      requireRole(role, ["owner", "admin"]);
      const renamed = await db
        .updateTable("orgs")
        .set({ name: orgName(name) })
        .where("id", "=", org.id)
        .returningAll()
        .executeTakeFirstOrThrow();
      return c.json(await orgBody(db, renamed, role));
    })
    .delete("/:orgId", perUser(limiter, "delete-org", 10), async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      await db.transaction().execute(async (trx) => {
        const { role } = await membership(trx, orgId, userId, { lock: true });
        requireRole(role, ["owner"]);
        if (await hasOpenSubscriptions(trx, orgId))
          throw conflict(
            "The org still has a subscription that has not ended; cancel it first.",
          );
        const members = await trx
          .selectFrom("org_users")
          .select("user_id")
          .where("org_id", "=", orgId)
          .execute();
        await trx.deleteFrom("orgs").where("id", "=", orgId).execute();
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "org.deleted",
          subject: orgId,
        });
        await seatsChanged(
          trx,
          members.map((member) => member.user_id),
        );
      });
      c.get("log").info({ orgId }, "deleted an org");
      return c.body(null, 204);
    })
    .get("/:orgId/members", async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      await membership(db, orgId, userId);
      const rows = await db
        .selectFrom("org_users")
        .where("org_id", "=", orgId)
        .select(["user_id", "role", "joined_at"])
        .orderBy("joined_at")
        .orderBy("user_id")
        .execute();
      const named = await people(
        c,
        rows.map((row) => row.user_id),
      );
      const body: OrgMemberList = {
        members: rows.map((row) => ({
          userId: row.user_id,
          role: row.role,
          name: named.get(row.user_id)?.name ?? null,
          imageUrl: named.get(row.user_id)?.imageUrl ?? null,
          joinedAt: row.joined_at.toISOString(),
        })),
      };
      return c.json(body);
    })
    .put(
      "/:orgId/members/:userId/role",
      perUser(limiter, "org-role", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const target = c.req.param("userId");
        const { role: next } = await readBody(c, "OrgRoleChange");
        const changed = await db.transaction().execute(async (trx) => {
          const { role } = await membership(trx, orgId, userId, {
            lock: true,
          });
          const current = await memberRole(trx, orgId, target);
          if (!manages(role, current) || !manages(role, next))
            throw forbidden(
              role === "user"
                ? "Only an org's owner or admin may change roles."
                : "Only an owner may make or unmake an owner.",
            );
          if (next !== "owner")
            await keepAnOwner(
              trx,
              orgId,
              current,
              "That would leave the org with no owner.",
            );
          await audit(trx, {
            userId: target,
            actor: `user:${userId}`,
            action: "org.role_changed",
            subject: orgId,
            detail: { from: current, to: next },
          });
          return trx
            .updateTable("org_users")
            .set({ role: next })
            .where("org_id", "=", orgId)
            .where("user_id", "=", target)
            .returning(["user_id", "role", "joined_at"])
            .executeTakeFirstOrThrow();
        });
        const person = (await people(c, [changed.user_id])).get(
          changed.user_id,
        );
        const body: OrgMember = {
          userId: changed.user_id,
          role: changed.role,
          name: person?.name ?? null,
          imageUrl: person?.imageUrl ?? null,
          joinedAt: changed.joined_at.toISOString(),
        };
        return c.json(body);
      },
    )
    .delete(
      "/:orgId/members/:userId",
      perUser(limiter, "remove-org-member", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const target = c.req.param("userId");
        await db.transaction().execute(async (trx) => {
          const { role } = await membership(trx, orgId, userId, {
            lock: true,
          });
          const current = await memberRole(trx, orgId, target);
          if (target !== userId && !manages(role, current))
            throw forbidden(
              role === "user"
                ? "Only an org's owner or admin may remove people."
                : "Only an owner may remove an owner.",
            );
          await keepAnOwner(
            trx,
            orgId,
            current,
            "That would leave the org with no owner.",
          );
          await trx
            .deleteFrom("org_users")
            .where("org_id", "=", orgId)
            .where("user_id", "=", target)
            .execute();
          await audit(trx, {
            userId: target,
            actor: `user:${userId}`,
            action: target === userId ? "org.left" : "org.member_removed",
            subject: orgId,
            detail: { role: current },
          });
          await seatsChanged(trx, [target]);
        });
        c.get("log").info({ orgId }, "removed an org member");
        return c.body(null, 204);
      },
    )
    .post("/:orgId/leave", perUser(limiter, "leave-org", 10), async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      await db.transaction().execute(async (trx) => {
        const { role } = await membership(trx, orgId, userId, { lock: true });
        await keepAnOwner(
          trx,
          orgId,
          role,
          "You are the org's only owner: make someone else an owner, or delete the org.",
        );
        await trx
          .deleteFrom("org_users")
          .where("org_id", "=", orgId)
          .where("user_id", "=", userId)
          .execute();
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "org.left",
          subject: orgId,
          detail: { role },
        });
        await seatsChanged(trx, [userId]);
      });
      c.get("log").info({ orgId }, "left an org");
      return c.body(null, 204);
    })
    .post(
      "/:orgId/invites",
      perUser(limiter, "invite-to-org", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const { role: invited } = await readBody(c, "OrgInviteCreate");
        const token = randomBytes(32).toString("base64url");
        const invite = await db.transaction().execute(async (trx) => {
          const { role } = await membership(trx, orgId, userId, {
            lock: true,
          });
          requireRole(role, ["owner", "admin"]);
          if (!manages(role, invited))
            throw forbidden("Only an owner may invite an owner.");
          if ((await seatsHeld(trx, orgId)) >= (await seatLimit(trx, orgId)))
            throw conflict(
              "Every seat is held by a member or a pending invite; add seats to the Team plan first.",
            );
          const created = await trx
            .insertInto("org_invites")
            .values({
              org_id: orgId,
              role: invited,
              token_hash: tokenHash(token),
              invited_by: userId,
              expires_at: sql<Date>`now() + make_interval(days => ${INVITE_DAYS})`,
            })
            .returningAll()
            .executeTakeFirstOrThrow();
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.invite_created",
            subject: orgId,
            detail: { inviteId: created.id, role: invited },
          });
          return created;
        });
        c.get("log").info({ orgId, inviteId: invite.id }, "invited to an org");
        const body: OrgInviteCreated = {
          invite: inviteBody(invite),
          token,
          link: `${appOrigin}/orgs/join#${token}`,
        };
        return c.json(body, 201);
      },
    )
    .get("/:orgId/invites", async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      const { role } = await membership(db, orgId, userId);
      requireRole(role, ["owner", "admin"]);
      const invites = await openInvites(db, orgId)
        .selectAll()
        .orderBy("created_at", "desc")
        .orderBy("id")
        .execute();
      const body: OrgInviteList = { invites: invites.map(inviteBody) };
      return c.json(body);
    })
    .delete(
      "/:orgId/invites/:inviteId",
      perUser(limiter, "revoke-org-invite", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const inviteId = c.req.param("inviteId");
        if (!UUID.test(inviteId))
          throw notFound("The org has no invite with that id.");
        await db.transaction().execute(async (trx) => {
          const { role } = await membership(trx, orgId, userId, {
            lock: true,
          });
          requireRole(role, ["owner", "admin"]);
          const invite = await trx
            .selectFrom("org_invites")
            .select(["role", "accepted_at", "revoked_at"])
            .where("id", "=", inviteId)
            .where("org_id", "=", orgId)
            .executeTakeFirst();
          if (!invite) throw notFound("The org has no invite with that id.");
          if (!manages(role, invite.role))
            throw forbidden(
              "Only an owner may revoke an invite to be an owner.",
            );
          if (invite.accepted_at || invite.revoked_at) return;
          await trx
            .updateTable("org_invites")
            .set({ revoked_at: sql<Date>`now()` })
            .where("id", "=", inviteId)
            .execute();
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.invite_revoked",
            subject: orgId,
            detail: { inviteId, role: invite.role },
          });
        });
        return c.body(null, 204);
      },
    );
}
