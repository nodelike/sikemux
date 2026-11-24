import type { Services } from "../app.ts";
import { Hono } from "hono";
import { sql } from "kysely";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import { forbidden, readBody } from "../http.ts";
import { perUser } from "../limits.ts";
import { followingHolds } from "../billing/team.ts";
import { appendEvents } from "../events/log.ts";

/** PUT /v1/account/followed-releases: a host says which releases it follows, in a request it signs. */
export function followedRoutes({
  database: { db },
  verifier,
  limiter,
}: Pick<Services, "database" | "verifier" | "limiter">) {
  return new Hono<AuthEnv>().put(
    "/",
    requireIdentity(verifier, db, { accept: ["host"] }),
    perUser(limiter, "followed-releases", 60),
    async (c) => {
      const { userId, deviceKey } = c.get("identity");
      if (!deviceKey) throw forbidden("Only a host follows releases.");
      const { channel } = await readBody(c, "FollowedReleases");
      await db.transaction().execute(async (trx) => {
        const host = await trx
          .selectFrom("devices")
          .select("followed_channel")
          .select(followingHolds.as("holds"))
          .where("key", "=", deviceKey)
          .where("user_id", "=", userId)
          .forUpdate()
          .executeTakeFirst();
        await trx
          .updateTable("devices")
          .set({
            followed_channel: channel,
            followed_at: sql<Date>`now()`,
          })
          .where("key", "=", deviceKey)
          .where("user_id", "=", userId)
          .execute();
        if (!host?.holds || host.followed_channel !== channel)
          await appendEvents(trx, [
            { userId, type: "plan.changed" },
            { userId, type: "orgs.changed" },
            { userId, type: "shares.changed" },
          ]);
      });
      return c.body(null, 204);
    },
  );
}
