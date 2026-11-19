import type { OrgTeam, OrgTeamList } from "@sikemux/protocol";
import type { Context } from "hono";
import { Hono } from "hono";
import type { Kysely, Selectable } from "kysely";

import { audit } from "../audit.ts";
import type { AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { badRequest, conflict, notFound, readBody } from "../http.ts";
import { perUser, type RateLimiter } from "../limits.ts";
import {
  idParam,
  memberOf,
  membership,
  orgIdOf,
  requireRole,
  requireTeamPlan,
} from "./shared.ts";

const NO_TEAM = "The org has no team with that id.";

function teamName(name: string) {
  const trimmed = name.trim();
  if (!trimmed) throw badRequest("A team's name cannot be blank.");
  return trimmed;
}

const isNameTaken = (error: unknown) =>
  (error as { code?: string }).code === "23505";

async function teamsBody(
  db: Kysely<Tables>,
  teams: Selectable<Tables["org_teams"]>[],
): Promise<OrgTeam[]> {
  const ids = teams.map((team) => team.id);
  const members = ids.length
    ? await db
        .selectFrom("org_team_members")
        .select(["team_id", "member_id"])
        .where("team_id", "in", ids)
        .orderBy("added_at")
        .orderBy("member_id")
        .execute()
    : [];
  return teams.map((team) => ({
    id: team.id,
    name: team.name,
    members: members
      .filter((row) => row.team_id === team.id)
      .map((row) => row.member_id),
    createdAt: team.created_at.toISOString(),
  }));
}

/** The team a request names, in the org it names. */
async function teamOf(c: Context, db: Kysely<Tables>, orgId: string) {
  const team = await db
    .selectFrom("org_teams")
    .selectAll()
    .where("org_id", "=", orgId)
    .where("id", "=", idParam(c, "teamId", NO_TEAM))
    .executeTakeFirst();
  if (!team) throw notFound(NO_TEAM);
  return team;
}

/** An org's teams: named lists of its members that owners and admins keep. */
export function teamRoutes(db: Kysely<Tables>, limiter: RateLimiter) {
  /** Locks the org, checks the caller runs it and its plan is live, and finds the team. */
  const managing = async (
    c: Context<AuthEnv>,
    trx: Kysely<Tables>,
    orgId: string,
  ) => {
    const { role } = await membership(trx, orgId, c.get("identity").userId, {
      lock: true,
    });
    requireRole(role, ["owner", "admin"]);
    await requireTeamPlan(trx, orgId);
  };

  const oneTeam = async (team: Selectable<Tables["org_teams"]>) =>
    (await teamsBody(db, [team]))[0];

  return new Hono<AuthEnv>()
    .get("/:orgId/teams", async (c) => {
      const orgId = orgIdOf(c);
      await membership(db, orgId, c.get("identity").userId);
      const teams = await db
        .selectFrom("org_teams")
        .selectAll()
        .where("org_id", "=", orgId)
        .orderBy("name")
        .execute();
      const body: OrgTeamList = { teams: await teamsBody(db, teams) };
      return c.json(body);
    })
    .post("/:orgId/teams", perUser(limiter, "org-team", 60), async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      const { name } = await readBody(c, "OrgTeamCreate");
      const team = await db
        .transaction()
        .execute(async (trx) => {
          await managing(c, trx, orgId);
          const created = await trx
            .insertInto("org_teams")
            .values({ org_id: orgId, name: teamName(name) })
            .returningAll()
            .executeTakeFirstOrThrow();
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.team_created",
            subject: orgId,
            detail: { teamId: created.id, team: created.name },
          });
          return created;
        })
        .catch((error: unknown) => {
          if (isNameTaken(error))
            throw conflict("The org has a team by that name already.");
          throw error;
        });
      return c.json(await oneTeam(team), 201);
    })
    .post(
      "/:orgId/teams/:teamId/name",
      perUser(limiter, "org-team", 60),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const { name } = await readBody(c, "OrgTeamRename");
        const team = await db
          .transaction()
          .execute(async (trx) => {
            await managing(c, trx, orgId);
            const before = await teamOf(c, trx, orgId);
            const renamed = await trx
              .updateTable("org_teams")
              .set({ name: teamName(name) })
              .where("id", "=", before.id)
              .returningAll()
              .executeTakeFirstOrThrow();
            if (renamed.name !== before.name)
              await audit(trx, {
                userId,
                actor: `user:${userId}`,
                action: "org.team_renamed",
                subject: orgId,
                detail: {
                  teamId: renamed.id,
                  team: renamed.name,
                  previousName: before.name,
                },
              });
            return renamed;
          })
          .catch((error: unknown) => {
            if (isNameTaken(error))
              throw conflict("The org has a team by that name already.");
            throw error;
          });
        return c.json(await oneTeam(team));
      },
    )
    .delete(
      "/:orgId/teams/:teamId",
      perUser(limiter, "org-team", 60),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        await db.transaction().execute(async (trx) => {
          await managing(c, trx, orgId);
          const team = await teamOf(c, trx, orgId);
          await trx.deleteFrom("org_teams").where("id", "=", team.id).execute();
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.team_deleted",
            subject: orgId,
            detail: { teamId: team.id, team: team.name },
          });
        });
        return c.body(null, 204);
      },
    )
    .put(
      "/:orgId/teams/:teamId/members/:memberId",
      perUser(limiter, "org-team", 60),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const team = await db.transaction().execute(async (trx) => {
          await managing(c, trx, orgId);
          const team = await teamOf(c, trx, orgId);
          const member = await memberOf(c, trx, orgId);
          const added = await trx
            .insertInto("org_team_members")
            .values({ team_id: team.id, member_id: member.id })
            .onConflict((oc) => oc.doNothing())
            .executeTakeFirst();
          if (added.numInsertedOrUpdatedRows)
            await audit(trx, {
              userId: member.user_id,
              actor: `user:${userId}`,
              action: "org.team_member_added",
              subject: orgId,
              detail: { teamId: team.id, team: team.name },
            });
          return team;
        });
        return c.json(await oneTeam(team));
      },
    )
    .delete(
      "/:orgId/teams/:teamId/members/:memberId",
      perUser(limiter, "org-team", 60),
      async (c) => {
        const { userId } = c.get("identity");
        const orgId = orgIdOf(c);
        const team = await db.transaction().execute(async (trx) => {
          await managing(c, trx, orgId);
          const team = await teamOf(c, trx, orgId);
          const member = await memberOf(c, trx, orgId);
          const removed = await trx
            .deleteFrom("org_team_members")
            .where("team_id", "=", team.id)
            .where("member_id", "=", member.id)
            .executeTakeFirst();
          if (removed.numDeletedRows)
            await audit(trx, {
              userId: member.user_id,
              actor: `user:${userId}`,
              action: "org.team_member_removed",
              subject: orgId,
              detail: { teamId: team.id, team: team.name },
            });
          return team;
        });
        return c.json(await oneTeam(team));
      },
    );
}
