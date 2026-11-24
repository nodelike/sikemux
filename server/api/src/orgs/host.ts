import type { Services } from "../app.ts";
import type { OrgMemberships } from "@sikemux/protocol";
import { Hono } from "hono";
import type { Kysely } from "kysely";

import { isStaff } from "../account/staff.ts";
import { requireIdentity, type AuthEnv } from "../auth.ts";
import {
  mayUseTeam,
  teamActive,
  teamAudience,
  teamFree,
} from "../billing/team.ts";
import type { Tables } from "../db.ts";
import { perUser } from "../limits.ts";
import { policyBody } from "./policy.ts";

/**
 * What a host shows of the account's orgs, by the releases it follows. A staff account's hosts
 * show them all. A host that follows stable keeps what it holds and shows nothing; one that never
 * said is from before it could, and is told only of orgs whose vault key it holds, to keep it.
 */
async function showing(
  db: Kysely<Tables>,
  userId: string,
  deviceKey: string | undefined,
): Promise<"shown" | "hidden" | "unsaid"> {
  if ((await isStaff(db, userId)) || (await teamAudience(db)) === "everyone")
    return "shown";
  const host = await db
    .selectFrom("devices")
    .select("followed_channel")
    .where("key", "=", deviceKey ?? "")
    .executeTakeFirst();
  if (!host?.followed_channel) return "unsaid";
  return host.followed_channel === "stable" ? "hidden" : "shown";
}

/**
 * GET /v1/account/orgs: the orgs the account a host is on is in, read again on orgs.changed. One
 * the account may not use Team in now reads as lapsed, so its hosts keep what they hold of it.
 */
export function membershipRoutes({
  database: { db },
  verifier,
  limiter,
}: Pick<Services, "database" | "verifier" | "limiter">) {
  return new Hono<AuthEnv>().get(
    "/",
    requireIdentity(verifier, db, { accept: ["host"] }),
    perUser(limiter, "account-orgs", 30),
    async (c) => {
      const { userId, deviceKey } = c.get("identity");
      const show = await showing(db, userId, deviceKey);
      const open = await mayUseTeam(db, userId);
      const rows = await db
        .selectFrom("orgs")
        .innerJoin("org_users", "org_users.org_id", "orgs.id")
        .select([
          "orgs.id",
          "orgs.name",
          "org_users.role",
          "orgs.rule_adding",
          "orgs.asking",
          "orgs.daily_questions",
        ])
        .select((eb) => [teamActive(eb).as("active"), teamFree(eb).as("free")])
        .where("org_users.user_id", "=", userId)
        .$if(show === "unsaid", (query) =>
          query.where("orgs.id", "in", (eb) =>
            eb
              .selectFrom("org_vault_keys")
              .select("org_id")
              .where("host_key", "=", deviceKey ?? ""),
          ),
        )
        .orderBy("orgs.created_at")
        .orderBy("orgs.id")
        .execute();
      const body: OrgMemberships = {
        orgs: rows.map((row) => ({
          id: row.id,
          name: row.name,
          role: row.role,
          plan: open && row.active ? "active" : "lapsed",
          policy: policyBody(row),
          ...(row.free ? { preview: true } : {}),
          ...(show !== "shown" ? { hidden: true } : {}),
        })),
      };
      return c.json(body);
    },
  );
}
