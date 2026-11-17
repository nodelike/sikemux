import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";

const USES_KEPT_DAYS = 90;

/** Drops the vault uses no one has made for 90 days. */
export async function purgeVaultUses(
  db: Kysely<Tables>,
  log: Logger,
): Promise<void> {
  const dropped = await db
    .deleteFrom("vault_uses")
    .where(
      "last_at",
      "<",
      sql<Date>`now() - make_interval(days => ${USES_KEPT_DAYS})`,
    )
    .executeTakeFirst();
  if (dropped.numDeletedRows > 0n)
    log.info(
      { uses: Number(dropped.numDeletedRows) },
      "dropped old vault uses",
    );
}
