import type { Kysely, Updateable } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import type { Logger } from "../log.ts";

export const TEAM_AUDIENCES = ["staff", "nightly", "everyone"] as const;
export type TeamAudience = (typeof TEAM_AUDIENCES)[number];

export const TEAM_VAULTS = ["shut", "open"] as const;
export type TeamVaults = (typeof TEAM_VAULTS)[number];

/** Opens Team to `audience`, or shuts it to the rest. */
export async function setTeamAudience(
  db: Kysely<Tables>,
  audience: TeamAudience,
  log: Logger,
): Promise<void> {
  const members = await set(db, { audience });
  log.info({ audience, members }, "set who Team is open to");
}

/** Opens orgs' vaults to everyone Team is open to, or shuts them to all but staff. */
export async function setTeamVaults(
  db: Kysely<Tables>,
  vaults: TeamVaults,
  log: Logger,
): Promise<void> {
  const members = await set(db, { vaults_open: vaults === "open" });
  log.info({ vaults, members }, "set whether orgs' vaults are open");
}

/** Has everyone in an org look again, and answers how many that is. */
function set(db: Kysely<Tables>, change: Updateable<Tables["team_audience"]>) {
  return db.transaction().execute(async (trx) => {
    await trx.updateTable("team_audience").set(change).execute();
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
}
