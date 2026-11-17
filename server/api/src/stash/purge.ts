import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import type { Logger } from "../log.ts";
import { removeStashFiles, sweepStashFiles } from "./files.ts";
import { TAKEN_KEPT_DAYS, UNFINISHED_PROMPT_DAYS } from "./items.ts";

const BATCH = 1_000;

/**
 * Deletes items taken more than 30 days ago, telling their accounts, and prompts whose
 * attachments never all arrived, then every file no item or mod holds: those of the attachments
 * deleted with their prompts, and of accounts purged before this ran.
 */
export async function purgeStash(
  db: Kysely<Tables>,
  dir: string | null,
  log: Logger,
): Promise<void> {
  let taken = 0;
  for (;;) {
    const gone = await db.transaction().execute(async (trx) => {
      const gone = await trx
        .deleteFrom("stash_items")
        .where(
          "id",
          "in",
          trx
            .selectFrom("stash_items")
            .select("id")
            .where("prompt_id", "is", null)
            .where(
              "taken_at",
              "<",
              sql<Date>`now() - make_interval(days => ${TAKEN_KEPT_DAYS})`,
            )
            .limit(BATCH),
        )
        .returning(["id", "user_id", "size"])
        .execute();
      await appendEvents(
        trx,
        gone.map((row) => ({
          userId: row.user_id,
          type: "stash.removed" as const,
          subject: row.id,
        })),
      );
      return gone;
    });
    taken += gone.length;
    if (dir)
      await removeStashFiles(
        dir,
        gone.filter((row) => row.size !== null).map((row) => row.id),
      );
    if (gone.length < BATCH) break;
  }
  if (taken) log.info({ items: taken }, "purged taken stash items");
  const unfinished = await db
    .deleteFrom("stash_items")
    .where("attachments_missing", ">", 0)
    .where(
      "created_at",
      "<",
      sql<Date>`now() - make_interval(days => ${UNFINISHED_PROMPT_DAYS})`,
    )
    .executeTakeFirst();
  if (unfinished.numDeletedRows > 0n)
    log.info(
      { prompts: Number(unfinished.numDeletedRows) },
      "purged prompts whose attachments never arrived",
    );
  if (!dir) return;
  await sweepStashFiles(
    dir,
    async (ids) =>
      new Set(
        (
          await db
            .selectFrom("stash_items")
            .select("id")
            .where("id", "in", ids)
            .union(
              db
                .selectFrom("mod_files")
                .select("stored_as as id")
                .where("stored_as", "in", ids),
            )
            .execute()
        ).map((row) => row.id),
      ),
    log,
  );
}
