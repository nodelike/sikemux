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
import { avatarSeed } from "@sikemux/protocol/avatar";
import type { Context } from "hono";
import { Hono } from "hono";
import { sql, type Kysely, type Selectable } from "kysely";

import type { Services } from "../app.ts";
import { audit } from "../audit.ts";
import type { Person } from "../account/clerk.ts";
import { dropSeatProofs } from "../account/proofs.ts";
import { isStaff } from "../account/staff.ts";
import { holdStanding, requireIdentity, type AuthEnv } from "../auth.ts";
import { makeDueCalls } from "../billing/calls.ts";
import { payerLeft } from "../billing/payer.ts";
import {
  hasOpenSubscriptions,
  memberCount,
  openInvites,
  seatLimit,
  seatsHeld,
} from "../billing/seats.ts";
import { PREVIEW_SEATS, requireTeam } from "../billing/team.ts";
import type { Tables } from "../db.ts";
import {
  ApiFailure,
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
import { normalizeInviteEmail } from "./mail.ts";
import { policyRoutes } from "./policy.ts";
import { manages } from "./roles.ts";
import {
  admit,
  idParam,
  inviteTokenHash,
  memberOf,
  membership,
  membersOf,
  orgBodies,
  orgBody,
  orgIdOf,
  peopleNamer,
  requireRole,
  requireTeamPlan,
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
    ...(invite.email !== null ? { email: invite.email } : {}),
    ...(invite.sent_at !== null
      ? { sentAt: invite.sent_at.toISOString() }
      : {}),
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
    avatarSeed: avatarSeed(member.user_id),
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

/** Staff may make as many as their tests take. */
async function refuseSecondPreview(db: Kysely<Tables>, userId: string) {
  if (await isStaff(db, userId)) return;
  const made = await db
    .selectFrom("orgs")
    .select("name")
    .where("created_by", "=", userId)
    .where("preview_seats", "is not", null)
    .executeTakeFirst();
  if (made)
    throw conflict(
      `The preview gives each person one org, and you made ${made.name}.`,
    );
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
  inviteMail,
}: Pick<
  Services,
  | "database"
  | "verifier"
  | "limiter"
  | "clerk"
  | "appOrigin"
  | "billing"
  | "txtRecords"
  | "inviteMail"
>) {
  const people = peopleNamer(clerk);

  /** Makes at once what a departing payer left for the provider, rather than at the next sweep. */
  const callProvider = (c: Context<AuthEnv>) =>
    provider ? makeDueCalls(db, provider, c.get("log")) : Promise.resolve();

  return new Hono<AuthEnv>()
    .use(requireIdentity(verifier, db))
    .use(requireTeam(db, live))
    .post("/", perUser(limiter, "create-org", 10), async (c) => {
      const { userId } = c.get("identity");
      const { name } = await readBody(c, "OrgCreate");
      const org = await db.transaction().execute(async (trx) => {
        await holdStanding(trx, userId, { exclusive: true });
        if (!live) await refuseSecondPreview(trx, userId);
        const created = await trx
          .insertInto("orgs")
          .values({
            name: orgName(name),
            created_by: userId,
            preview_seats: live ? null : PREVIEW_SEATS,
          })
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
        orgs: await orgBodies(db, rows),
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
        await holdStanding(trx, userId);
        await trx
          .selectFrom("orgs")
          .select("id")
          .where("id", "=", invite.org_id)
          .forUpdate()
          .execute();
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
      await callProvider(c);
      return c.json(await orgBody(db, joined.org, joined.role));
    })
    .post("/:orgId/name", perUser(limiter, "rename-org", 30), async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      const { name } = await readBody(c, "OrgRename");
      const to = orgName(name);
      const { org, role } = await db.transaction().execute(async (trx) => {
        const member = await membership(trx, orgId, userId, { lock: true });
        requireRole(member.role, ["owner", "admin"]);
        await requireTeamPlan(trx, orgId);
        if (member.org.name === to) return member;
        const renamed = await trx
          .updateTable("orgs")
          .set({ name: to })
          .where("id", "=", orgId)
          .returningAll()
          .executeTakeFirstOrThrow();
        await audit(trx, {
          userId,
          actor: `user:${userId}`,
          action: "org.renamed",
          subject: orgId,
          detail: { name: to, previousName: member.org.name },
        });
        await orgsChanged(trx, await membersOf(trx, orgId));
        return { org: renamed, role: member.role };
      });
      return c.json(await orgBody(db, org, role));
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
      await callProvider(c);
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
          await orgsChanged(trx, await membersOf(trx, orgId));
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
        await orgsChanged(trx, await membersOf(trx, orgId));
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
        const { role: invited, email: rawEmail } = await readBody(
          c,
          "OrgInviteCreate",
        );
        const email =
          rawEmail !== undefined ? normalizeInviteEmail(rawEmail) : null;
        if (email !== null && !inviteMail)
          throw new ApiFailure(
            503,
            "unavailable",
            "Email invites are not configured",
          );
        const token = randomBytes(32).toString("base64url");
        const { invite, orgName: targetOrgName } = await db
          .transaction()
          .execute(async (trx) => {
            const { org, role } = await membership(trx, orgId, userId, {
              lock: true,
            });
            requireRole(role, ["owner", "admin"]);
            if (!manages(role, invited))
              throw forbidden("Only an owner may invite an owner.");
            if (email !== null) {
              const duplicate = await openInvites(trx, orgId)
                .select("id")
                .where("email", "=", email)
                .executeTakeFirst();
              if (duplicate)
                throw conflict(
                  "There is already a pending invite for that email address.",
                );
            }
            if ((await seatsHeld(trx, orgId)) >= (await seatLimit(trx, orgId)))
              throw conflict(
                "Every seat is held by a member or a pending invite; add seats to the Team plan first.",
              );
            const created = await trx
              .insertInto("org_invites")
              .values({
                org_id: orgId,
                role: invited,
                email,
                sent_at: email !== null ? sql<Date>`now()` : null,
                token_hash: inviteTokenHash(token),
                invited_by: userId,
                expires_at: sql<Date>`now() + make_interval(days => ${INVITE_DAYS})`,
              })
              .returningAll()
              .executeTakeFirstOrThrow();
            if (email === null) {
              await audit(trx, {
                userId,
                actor: `user:${userId}`,
                action: "org.invite_created",
                subject: orgId,
                detail: { inviteId: created.id, role: invited },
              });
            }
            return { invite: created, orgName: org.name };
          });
        const named = await people(c, [userId]);
        const link = `${appOrigin}/orgs/join#${token}`;
        if (email !== null && inviteMail) {
          try {
            await inviteMail.sendInvite(
              {
                to: email,
                orgName: targetOrgName,
                invitedBy: named.get(userId)?.name ?? null,
                role: invited,
                expiresAt: invite.expires_at,
                link,
              },
              c.get("log"),
            );
          } catch (error) {
            await db
              .deleteFrom("org_invites")
              .where("id", "=", invite.id)
              .where("accepted_at", "is", null)
              .execute()
              .catch((cleanupError: unknown) =>
                c
                  .get("log")
                  .error(
                    { err: cleanupError, orgId, inviteId: invite.id },
                    "failed to roll back unsent org invite",
                  ),
              );
            throw error;
          }
          await audit(db, {
            userId,
            actor: `user:${userId}`,
            action: "org.invite_created",
            subject: orgId,
            detail: { inviteId: invite.id, role: invited },
          });
        }
        c.get("log").info({ orgId, inviteId: invite.id }, "invited to an org");
        const body: OrgInviteCreated = {
          invite: inviteBody(invite, named),
          token,
          link,
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
    .post(
      "/:orgId/invites/:inviteId/resend",
      perUser(limiter, "resend-org-invite", 10),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const inviteId = idParam(
          c,
          "inviteId",
          "The org has no pending invite with that id.",
        );
        if (!inviteMail)
          throw new ApiFailure(
            503,
            "unavailable",
            "Email invites are not configured",
          );
        const token = randomBytes(32).toString("base64url");
        const newHash = inviteTokenHash(token);
        const {
          updated,
          previous,
          orgName: targetOrgName,
          email,
        } = await db.transaction().execute(async (trx) => {
          const { org, role } = await membership(trx, orgId, userId, {
            lock: true,
          });
          requireRole(role, ["owner", "admin"]);
          const existing = await openInvites(trx, orgId)
            .selectAll()
            .where("id", "=", inviteId)
            .executeTakeFirst();
          if (!existing)
            throw notFound("The org has no pending invite with that id.");
          if (!manages(role, existing.role))
            throw forbidden(
              "Only an owner may resend an invite to be an owner.",
            );
          if (existing.email === null)
            throw badRequest("Only email invites can be resent.");
          const refreshed = await trx
            .updateTable("org_invites")
            .set({
              token_hash: newHash,
              sent_at: sql<Date>`now()`,
              expires_at: sql<Date>`now() + make_interval(days => ${INVITE_DAYS})`,
            })
            .where("id", "=", inviteId)
            .returningAll()
            .executeTakeFirstOrThrow();
          return {
            updated: refreshed,
            previous: {
              token_hash: existing.token_hash,
              sent_at: existing.sent_at,
              expires_at: existing.expires_at,
            },
            orgName: org.name,
            email: existing.email,
          };
        });
        const named = await people(c, [
          ...new Set([updated.invited_by, userId]),
        ]);
        const link = `${appOrigin}/orgs/join#${token}`;
        try {
          await inviteMail.sendInvite(
            {
              to: email,
              orgName: targetOrgName,
              invitedBy:
                named.get(userId)?.name ??
                named.get(updated.invited_by)?.name ??
                null,
              role: updated.role,
              expiresAt: updated.expires_at,
              link,
            },
            c.get("log"),
          );
        } catch (error) {
          await db
            .updateTable("org_invites")
            .set(previous)
            .where("id", "=", inviteId)
            .where("token_hash", "=", newHash)
            .where("accepted_at", "is", null)
            .execute()
            .catch((cleanupError: unknown) =>
              c
                .get("log")
                .error(
                  { err: cleanupError, orgId, inviteId },
                  "failed to restore org invite after resend failure",
                ),
            );
          throw error;
        }
        c.get("log").info({ orgId, inviteId }, "resent an org invite");
        const body: OrgInviteCreated = {
          invite: inviteBody(updated, named),
          token,
          link,
        };
        return c.json(body);
      },
    )
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
    .route("/", domainRoutes(db, limiter, clerk, txtRecords, callProvider))
    .route("/", auditRoutes(db, people))
    .route("/", policyRoutes(db, limiter));
}
