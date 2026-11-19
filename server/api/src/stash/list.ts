import type { StashList, StashSeen, StashTaken } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import { holdStanding } from "../auth.ts";
import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { badRequest } from "../http.ts";
import { ITEM_ID, withAttachments } from "./items.ts";

const PAGE = 50;
const CURSOR_TIME =
  /^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(\.\d{1,6})?[+-]\d{2}(:\d{2})?$/;

function cursorOf(createdAt: string, id: string): string {
  return Buffer.from(`${createdAt}|${id}`).toString("base64url");
}

function readCursor(cursor: string): { createdAt: string; id: string } {
  const [createdAt = "", id = ""] = Buffer.from(cursor, "base64url")
    .toString("utf8")
    .split("|");
  if (!CURSOR_TIME.test(createdAt) || !ITEM_ID.test(id))
    throw badRequest("The cursor is not one this server gave.");
  return { createdAt, id };
}

/** A page of the account's stash, newest first, from `cursor` when given. */
export async function listStash(
  db: Kysely<Tables>,
  userId: string,
  taken: StashTaken,
  cursor: string | undefined,
): Promise<StashList> {
  let query = db
    .selectFrom("stash_items")
    .selectAll()
    .select(sql<string>`created_at::text`.as("created_text"))
    .where("user_id", "=", userId)
    .where("prompt_id", "is", null)
    .where("attachments_missing", "=", 0);
  if (taken === "false") query = query.where("taken_at", "is", null);
  if (taken === "true") query = query.where("taken_at", "is not", null);
  if (cursor) {
    const after = readCursor(cursor);
    query = query.where(
      sql<boolean>`(created_at, id) < (${after.createdAt}::timestamptz, ${after.id})`,
    );
  }
  const [rows, user] = await Promise.all([
    query
      .orderBy("created_at", "desc")
      .orderBy("id", "desc")
      .limit(PAGE + 1)
      .execute(),
    db
      .selectFrom("users")
      .select("stash_seen_at")
      .where("id", "=", userId)
      .executeTakeFirst(),
  ]);
  const page = rows.slice(0, PAGE);
  const last = page.at(-1);
  const list: StashList = {
    items: await withAttachments(db, page),
    seenAt: user?.stash_seen_at?.toISOString() ?? null,
  };
  if (rows.length > PAGE && last)
    list.cursor = cursorOf(last.created_text, last.id);
  return list;
}

/** Moves when the person last looked at the stash forward to `at`, never past now, telling every device. */
export async function seeStash(
  db: Kysely<Tables>,
  userId: string,
  at: string,
): Promise<StashSeen> {
  const seenAt = await db.transaction().execute(async (trx) => {
    await holdStanding(trx, userId, { exclusive: true });
    const moved = await trx
      .updateTable("users")
      .set({ stash_seen_at: sql<Date>`least(${at}::timestamptz, now())` })
      .where("id", "=", userId)
      .where((eb) =>
        eb.or([
          eb("stash_seen_at", "is", null),
          eb("stash_seen_at", "<", sql<Date>`least(${at}::timestamptz, now())`),
        ]),
      )
      .returning("stash_seen_at")
      .executeTakeFirst();
    if (moved) {
      await appendEvent(trx, { userId, type: "stash.seen" });
      return moved.stash_seen_at;
    }
    const user = await trx
      .selectFrom("users")
      .select("stash_seen_at")
      .where("id", "=", userId)
      .executeTakeFirstOrThrow();
    return user.stash_seen_at;
  });
  return { at: (seenAt ?? new Date(at)).toISOString() };
}
