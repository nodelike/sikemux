import type { OrgPolicy } from "@sikemux/protocol";
import { Hono } from "hono";
import type { Kysely, Selectable } from "kysely";

import { audit } from "../audit.ts";
import type { AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { readBody } from "../http.ts";
import { perUser, type RateLimiter } from "../limits.ts";
import {
  membership,
  orgIdOf,
  orgsChanged,
  requireRole,
  requireTeamPlan,
} from "./shared.ts";

export function policyBody(
  org: Pick<
    Selectable<Tables["orgs"]>,
    "rule_adding" | "asking" | "daily_questions"
  >,
): OrgPolicy {
  return {
    ruleAdding: org.rule_adding,
    asking: org.asking,
    dailyQuestions: org.daily_questions,
  };
}

/** How the org's agents work together: every member reads it, owners and admins change it. */
export function policyRoutes(db: Kysely<Tables>, limiter: RateLimiter) {
  return new Hono<AuthEnv>()
    .get("/:orgId/policy", async (c) => {
      const { org } = await membership(
        db,
        orgIdOf(c),
        c.get("identity").userId,
      );
      return c.json(policyBody(org));
    })
    .put("/:orgId/policy", perUser(limiter, "org-policy", 60), async (c) => {
      const { userId } = c.get("identity");
      const orgId = orgIdOf(c);
      const asked = await readBody(c, "OrgPolicy");
      const policy = await db.transaction().execute(async (trx) => {
        const { org, role } = await membership(trx, orgId, userId, {
          lock: true,
        });
        requireRole(role, ["owner", "admin"]);
        await requireTeamPlan(trx, orgId);
        const was = policyBody(org);
        const changed = (Object.keys(asked) as (keyof OrgPolicy)[]).filter(
          (key) => asked[key] !== was[key],
        );
        if (changed.length === 0) return was;
        await trx
          .updateTable("orgs")
          .set({
            rule_adding: asked.ruleAdding,
            asking: asked.asking,
            daily_questions: asked.dailyQuestions,
          })
          .where("id", "=", orgId)
          .execute();
        for (const key of changed)
          await audit(trx, {
            userId,
            actor: `user:${userId}`,
            action: "org.policy_changed",
            subject: orgId,
            detail: { [key]: asked[key] },
          });
        const members = await trx
          .selectFrom("org_users")
          .select("user_id")
          .where("org_id", "=", orgId)
          .execute();
        await orgsChanged(
          trx,
          members.map((member) => member.user_id),
        );
        return asked;
      });
      return c.json(policy);
    });
}
