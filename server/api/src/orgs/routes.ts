import { randomBytes } from "node:crypto";
import type {
  OrgInvite,
  OrgInviteCreated,
  OrgInviteList,
  OrgInvitePreview,
  OrgList,
  OrgMember,
  OrgMemberList,
  OrgRole,
} from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";
import { sql, type Kysely, type Selectable } from "kysely";

import type { Services } from "../app.ts";
import { audit } from "../audit.ts";
import type { Person } from "../account/clerk.ts";
import { dropSeatProofs } from "../account/proofs.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import { requireBillingAccess } from "../billing/access.ts";
import { makeDueCalls } from "../billing/calls.ts";
import { payerLeft } from "../billing/payer.ts";
import {
  hasOpenSubscriptions,
  memberCount,
  openInvites,
  seatLimit,
  seatsHeld,
} from "../billing/seats.ts";
import type { Tables } from "../db.ts";
import {
  badRequest,
  conflict,
  forbidden,
  notFound,
  readBody,
  type Env,
} from "../http.ts";
import { clientAddress, limit, perUser } from "../limits.ts";
import { dropUnshared } from "../orgvault/projects.ts";
import { domainRoutes } from "./domains.ts";
import { auditRoutes } from "./log.ts";
import { policyRoutes } from "./policy.ts";
import { manages } from "./roles.ts";
import {
  admit,
  idParam,
  inviteTokenHash,
  memberOf,
  membership,
  orgBody,
  orgIdOf,
  peopleNamer,
  requireRole,
  orgsChanged,
  seatsChanged,
} from "./shared.ts";
import { teamRoutes } from "./teams.ts";

const INVITE_DAYS = 7;

function orgName(name: string) {
  const trimmed = name.trim();
  if (!trimmed) throw badRequest("An org's name cannot be blank.");
  return trimmed;
}

function inviteBody(
  invite: Selectable<Tables["org_invites"]>,
  named: Map<string, Person>,
): OrgInvite {
  return {
    id: invite.id,
    role: invite.role,
    invitedBy: named.get(invite.invited_by)?.name ?? null,
    createdAt: invite.created_at.toISOString(),
    expiresAt: invite.expires_at.toISOString(),
  };
}

function memberBody(
  member: Pick<
    Selectable<Tables["org_users"]>,
    "id" | "user_id" | "role" | "joined_at"
  >,
  named: Map<string, Person>,
  caller: string,
): OrgMember {
  return {
    id: member.id,
    role: member.role,
    name: named.get(member.user_id)?.name ?? null,
    imageUrl: named.get(member.user_id)?.imageUrl ?? null,
    joinedAt: member.joined_at.toISOString(),
    you: member.user_id === caller,
  };
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

async function membersOf(db: Kysely<Tables>, orgId: string) {
  const rows = await db
    .selectFrom("org_users")
    .select("user_id")
    .where("org_id", "=", orgId)
    .execute();
  return rows.map((row) => row.user_id);
}

/** Names an invite's org for a page whose reader has not signed in yet; any link that would not join answers alike. */
export function invitePreviewRoutes({
  database: { db },
  limiter,
}: Pick<Services, "database" | "limiter">) {
  return new Hono<Env>().post(
    "/",
    limit<Env>(limiter, "preview-invite", 30, clientAddress),
    async (c) => {
      const { token } = await readBody(c, "OrgJoin");
      const invite = await db
        .selectFrom("org_invites")
        .innerJoin("orgs", "orgs.id", "org_invites.org_id")
        .select("orgs.name")
        .where("org_invites.token_hash", "=", inviteTokenHash(token))
        .where("org_invites.accepted_at", "is", null)
        .where("org_invites.revoked_at", "is", null)
        .where("org_invites.expires_at", ">", sql<Date>`now()`)
        .executeTakeFirst();
      const body: OrgInvitePreview = { orgName: invite?.name ?? null };
      return c.json(body);
    },
  );
}

export function orgRoutes({
  database: { db },
  verifier,
  limiter,
  clerk,
  appOrigin,
  billing: { live, provider },
  txtRecords,
}: Pick<
  Services,
  | "database"
  | "verifier"
  | "limiter"
  | "clerk"
  | "appOrigin"
  | "billing"
  | "txtRecords"
>) {
  const people = peopleNamer(clerk);

  /** Makes at once what a departing payer left for the provider, rather than at the next sweep. */
  const callProvider = (c: Context<AuthEnv>) =>
    provider ? makeDueCalls(db, provider, c.get("log")) : Promise.resolve();

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
        await orgsChanged(trx, [userId]);
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
          .where("token_hash", "=", inviteTokenHash(token))
          .where("accepted_at", "is", null)
          .where("revoked_at", "is", null)
          .where("expires_at", ">", sql<Date>`now()`)
          .executeTakeFirst();
        if (!invite)
          throw notFound(
            "No invite has that link, or it was used, revoked or has expired.",
          );
        const used = await trx
          .updateTable("org_invites")
          .set({ accepted_by: userId, accepted_at: sql<Date>`now()` })
          .where("id", "=", invite.id)
          .where("accepted_at", "is", null)
          .where("revoked_at", "is", null)
          .executeTakeFirst();
        if (used.numUpdatedRows === 0n)
          throw notFound("That invite was just used or revoked.");
        const org = await admit(trx, {
          orgId: invite.org_id,
          userId,
          role: invite.role,
          invitedBy: invite.invited_by,
          detail: { via: "invite", inviteId: invite.id },
        });
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
      const renamed = await db.transaction().execute(async (trx) => {
        const row = await trx
          .updateTable("orgs")
          .set({ name: orgName(name) })
          .where("id", "=", org.id)
          .returningAll()
          .executeTakeFirstOrThrow();
        await orgsChanged(trx, await membersOf(trx, org.id));
        return row;
      });
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
        const members = await membersOf(trx, orgId);
        await dropSeatProofs(trx, orgId, members);
        await trx.deleteFrom("orgs").where("id", "=", orgId).execute();
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "org.deleted",
          subject: orgId,
        });
        await seatsChanged(trx, members);
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
        .select(["id", "user_id", "role", "joined_at"])
        .orderBy("joined_at")
        .orderBy("user_id")
        .execute();
      const named = await people(
        c,
        rows.map((row) => row.user_id),
      );
      const body: OrgMemberList = {
        members: rows.map((row) => memberBody(row, named, userId)),
      };
      return c.json(body);
    })
    .put(
      "/:orgId/members/:memberId/role",
      perUser(limiter, "org-role", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const { role: next } = await readBody(c, "OrgRoleChange");
        const changed = await db.transaction().execute(async (trx) => {
          const { role } = await membership(trx, orgId, userId, {
            lock: true,
          });
          const { user_id: target, role: current } = await memberOf(
            c,
            trx,
            orgId,
          );
          if (!manages(role, current) || !manages(role, next))
            throw forbidden(
              role === "user"
                ? "Only an org's owner or admin may change roles."
                : "Only an owner may make or unmake an owner.",
            );
          if (next !== "owner") {
            await keepAnOwner(
              trx,
              orgId,
              current,
              "That would leave the org with no owner.",
            );
            await payerLeft(trx, orgId, target);
          }
          await audit(trx, {
            userId: target,
            actor: `user:${userId}`,
            action: "org.role_changed",
            subject: orgId,
            detail: { from: current, to: next },
          });
          await orgsChanged(trx, [target]);
          return trx
            .updateTable("org_users")
            .set({ role: next })
            .where("org_id", "=", orgId)
            .where("user_id", "=", target)
            .returning(["id", "user_id", "role", "joined_at"])
            .executeTakeFirstOrThrow();
        });
        await callProvider(c);
        return c.json(
          memberBody(changed, await people(c, [changed.user_id]), userId),
        );
      },
    )
    .delete(
      "/:orgId/members/:memberId",
      perUser(limiter, "remove-org-member", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        await db.transaction().execute(async (trx) => {
          const { role } = await membership(trx, orgId, userId, {
            lock: true,
          });
          const { user_id: target, role: current } = await memberOf(
            c,
            trx,
            orgId,
          );
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
          await payerLeft(trx, orgId, target);
          await dropSeatProofs(trx, orgId, [target]);
          await dropUnshared(trx, orgId);
          await audit(trx, {
            userId: target,
            actor: `user:${userId}`,
            action: target === userId ? "org.left" : "org.member_removed",
            subject: orgId,
            detail: { role: current },
          });
          await seatsChanged(trx, [target]);
        });
        await callProvider(c);
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
        await payerLeft(trx, orgId, userId);
        await dropSeatProofs(trx, orgId, [userId]);
        await dropUnshared(trx, orgId);
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "org.left",
          subject: orgId,
          detail: { role },
        });
        await seatsChanged(trx, [userId]);
      });
      await callProvider(c);
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
              token_hash: inviteTokenHash(token),
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
          invite: inviteBody(invite, await people(c, [userId])),
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
      const named = await people(c, [
        ...new Set(invites.map((invite) => invite.invited_by)),
      ]);
      const body: OrgInviteList = {
        invites: invites.map((invite) => inviteBody(invite, named)),
      };
      return c.json(body);
    })
    .delete(
      "/:orgId/invites/:inviteId",
      perUser(limiter, "revoke-org-invite", 30),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const inviteId = idParam(
          c,
          "inviteId",
          "The org has no invite with that id.",
        );
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
    )
    .route("/", teamRoutes(db, limiter))
    .route("/", domainRoutes(db, limiter, clerk, txtRecords))
    .route("/", auditRoutes(db, people))
    .route("/", policyRoutes(db, limiter));
}
