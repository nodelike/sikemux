import type { Services } from "../app.ts";
import type { OrgMemberships } from "@sikemux/protocol";
import { Hono } from "hono";
import { sql, type ExpressionBuilder } from "kysely";

import { isStaff } from "../account/staff.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { perUser } from "../limits.ts";

/**
 * GET /v1/account/orgs: the orgs the account a host is on is in, read again on orgs.changed.
 * Until plans go live only staff are in any.
 */
/** Whether the org in `orgs.id` holds an active Team subscription. */
export function teamPaid(eb: ExpressionBuilder<Tables, "orgs">) {
  return eb.exists(
    eb
      .selectFrom("subscriptions")
      .innerJoin(
        "billing_customers",
        "billing_customers.id",
        "subscriptions.customer_id",
      )
      .select(sql`1`.as("one"))
      .whereRef("billing_customers.org_id", "=", "orgs.id")
      .where("subscriptions.product", "=", "team")
      .where("subscriptions.status", "=", "active"),
  );
}

export function membershipRoutes({
  database: { db },
  verifier,
  limiter,
  billing: { live },
}: Pick<Services, "database" | "verifier" | "limiter" | "billing">) {
  return new Hono<AuthEnv>().get(
    "/",
    requireIdentity(verifier, db, { accept: ["host"] }),
    perUser(limiter, "account-orgs", 30),
    async (c) => {
      const { userId } = c.get("identity");
      if (!live && !(await isStaff(db, userId)))
        return c.json({ orgs: [] } satisfies OrgMemberships);
      const rows = await db
        .selectFrom("orgs")
        .innerJoin("org_users", "org_users.org_id", "orgs.id")
        .select(["orgs.id", "orgs.name", "org_users.role"])
        .select((eb) => teamPaid(eb).as("paid"))
        .where("org_users.user_id", "=", userId)
        .orderBy("orgs.created_at")
        .orderBy("orgs.id")
        .execute();
      const body: OrgMemberships = {
        orgs: rows.map(({ id, name, role, paid }) => ({
          id,
          name,
          role,
          plan: paid ? "active" : "lapsed",
        })),
      };
      return c.json(body);
    },
  );
}
