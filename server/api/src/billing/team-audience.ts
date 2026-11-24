import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import type { Logger } from "../log.ts";

export const TEAM_AUDIENCES = ["staff", "nightly", "everyone"] as const;
export type TeamAudience = (typeof TEAM_AUDIENCES)[number];

/** Opens Team to `audience`, or shuts it to the rest, and has everyone in an org look again. */
export async function setTeamAudience(
  db: Kysely<Tables>,
  audience: TeamAudience,
  log: Logger,
): Promise<void> {
  const members = await db.transaction().execute(async (trx) => {
    await trx.updateTable("team_audience").set({ audience }).execute();
    const rows = await trx
      .selectFrom("org_users")
      .select("user_id")
      .distinct()
      .execute();
    await appendEvents(
      trx,
      rows.flatMap(({ user_id }) => [
        { userId: user_id, type: "plan.changed" as const },
        { userId: user_id, type: "orgs.changed" as const },
      ]),
    );
    return rows.length;
  });
  log.info({ audience, members }, "set who Team is open to");
}
