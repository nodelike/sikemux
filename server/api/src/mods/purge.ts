import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import type { Logger } from "../log.ts";

const REMOVALS_KEPT_DAYS = 90;

/**
 * Drops removal markers and mod purges older than 90 days. A host whose cursor is older than
 * the newest one dropped can no longer learn of it, so its next pull starts over.
 */
export async function purgeModRemovals(
  db: Kysely<Tables>,
  log: Logger,
): Promise<void> {
  const cutoff = sql<Date>`now() - make_interval(days => ${REMOVALS_KEPT_DAYS})`;
  const { rows } = await db.transaction().execute(async (trx) => {
    await sql`
      select 1 from mod_counters
      where user_id in (
        select user_id from mod_documents where removed and updated_at < ${cutoff}
        union
        select user_id from mod_purges where purged_at < ${cutoff}
      )
      order by user_id
      for update`.execute(trx);
    return sql<{ cleared: string }>`
      with documents as (
        delete from mod_documents
        where removed and updated_at < ${cutoff}
        returning user_id, seq, length(sealed) as bytes
      ), purges as (
        delete from mod_purges
        where purged_at < ${cutoff}
        returning user_id, seq, 0 as bytes
      ), cleared as (
        select user_id, max(seq) as seq, sum(bytes) as bytes, count(*) as cleared
        from (select * from documents union all select * from purges) as gone
        group by user_id
      )
      update mod_counters
      set cleared_seq = greatest(mod_counters.cleared_seq, cleared.seq),
        sealed_bytes = mod_counters.sealed_bytes - cleared.bytes
      from cleared
      where mod_counters.user_id = cleared.user_id
      returning cleared.cleared`.execute(trx);
  });
  const cleared = rows.reduce((sum, row) => sum + Number(row.cleared), 0);
  if (cleared > 0) log.info({ removals: cleared }, "dropped old mod removals");
}
