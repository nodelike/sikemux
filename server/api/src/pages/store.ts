import { createHash, randomBytes, timingSafeEqual } from "node:crypto";
import type { PageShare, PageShareState } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import { holdStanding } from "../auth.ts";
import type { Tables } from "../db.ts";
import { signInOf } from "../devices/register.ts";
import { ApiFailure, conflict, notFound } from "../http.ts";
import type { Logger } from "../log.ts";
import type { StashFiles } from "../stash/files.ts";
import { removeStashFiles } from "../stash/files.ts";

/** A sealed page as the host sends it: 25 MiB of base64url text. */
export const MAX_PAGE_BYTES = 25 * 1024 * 1024;
export const MAX_LIVE_PAGES = 50;
export const MAX_PAGE_DAYS = 30;
/** How long a page's row outlives its link, so its host can still read how it ended. */
const KEPT_AFTER_DAYS = 30;

export interface PageSettings {
  viewerCheck: string;
  anyone: boolean;
  expiresAt: Date;
}

interface PageRow {
  id: string;
  views: number;
  ended_at: Date | null;
  expires_at: Date;
}

function share(row: PageRow): PageShare {
  return { id: row.id, expiresAt: row.expires_at.getTime(), views: row.views };
}

function state(row: PageRow): PageShareState {
  return { ...share(row), ended: row.ended_at !== null };
}

/**
 * Keeps the sealed page under a new name in the stash's file store, then points the page at it,
 * so a viewer reads the old version or the new one whole. The old version's file goes after.
 */
export async function putPage(
  db: Kysely<Tables>,
  store: StashFiles,
  { userId, hostKey }: { userId: string; hostKey: string },
  id: string,
  settings: PageSettings,
  declared: number,
  body: AsyncIterable<Uint8Array>,
  checkBody: (sha256: string) => void,
  log: Logger,
): Promise<PageShare> {
  if (declared > MAX_PAGE_BYTES)
    throw new ApiFailure(
      413,
      "payload_too_large",
      `A sealed page is limited to ${MAX_PAGE_BYTES} bytes.`,
    );
  const name = randomBytes(16).toString("base64url");
  const written = await store.write(name, body, MAX_PAGE_BYTES);
  try {
    checkBody(written.sha256);
    const { row, replaced } = await db.transaction().execute(async (trx) => {
      await holdStanding(trx, userId, { exclusive: true });
      await signInOf(trx, userId, hostKey);
      const before = await trx
        .selectFrom("pages")
        .select(["user_id", "stored_as"])
        .where("id", "=", id)
        .forUpdate()
        .executeTakeFirst();
      if (before && before.user_id !== userId)
        throw conflict("Another account shares a page under that id.");
      const { live } = await trx
        .selectFrom("pages")
        .select((eb) => eb.fn.countAll<string>().as("live"))
        .where("user_id", "=", userId)
        .where("id", "!=", id)
        .where("ended_at", "is", null)
        .where("expires_at", ">", new Date())
        .executeTakeFirstOrThrow();
      if (Number(live) >= MAX_LIVE_PAGES)
        throw conflict(
          `An account shares at most ${MAX_LIVE_PAGES} pages at once. Stop one to share another.`,
        );
      const values = {
        stored_as: name,
        size: written.size,
        viewer_check: settings.viewerCheck,
        anyone: settings.anyone,
        expires_at: settings.expiresAt,
        stored_by: hostKey,
      };
      const row = before
        ? await trx
            .updateTable("pages")
            .set({ ...values, ended_at: null, updated_at: new Date() })
            .where("id", "=", id)
            .returning(["id", "views", "ended_at", "expires_at"])
            .executeTakeFirstOrThrow()
        : await trx
            .insertInto("pages")
            .values({ id, user_id: userId, ...values })
            .returning(["id", "views", "ended_at", "expires_at"])
            .executeTakeFirstOrThrow();
      await store.commit(name);
      return { row, replaced: before?.stored_as ?? null };
    });
    if (replaced)
      await removeStashFiles(store.dir, [replaced]).catch((error: unknown) =>
        log.warn({ err: error, id }, "could not delete a page's old version"),
      );
    return share(row);
  } catch (error) {
    await store.discard(name);
    throw error;
  }
}

export async function stopPage(
  db: Kysely<Tables>,
  store: StashFiles | null,
  userId: string,
  id: string,
): Promise<PageShareState> {
  const { row, stored } = await db.transaction().execute(async (trx) => {
    const before = await trx
      .selectFrom("pages")
      .select("stored_as")
      .where("id", "=", id)
      .where("user_id", "=", userId)
      .forUpdate()
      .executeTakeFirst();
    if (!before) throw notFound("The account shares no page with that id.");
    const row = await trx
      .updateTable("pages")
      .set({
        stored_as: null,
        ended_at: sql<Date>`coalesce(ended_at, now())`,
        updated_at: new Date(),
      })
      .where("id", "=", id)
      .returning(["id", "views", "ended_at", "expires_at"])
      .executeTakeFirstOrThrow();
    return { row, stored: before.stored_as };
  });
  if (stored && store) await removeStashFiles(store.dir, [stored]);
  return state(row);
}

export async function pageState(
  db: Kysely<Tables>,
  userId: string,
  id: string,
): Promise<PageShareState> {
  const row = await db
    .selectFrom("pages")
    .select(["id", "views", "ended_at", "expires_at"])
    .where("id", "=", id)
    .where("user_id", "=", userId)
    .executeTakeFirst();
  if (!row) throw notFound("The account shares no page with that id.");
  return state(row);
}

/** Whether `proof`, the link's HMAC in hex, is the one the host's check was taken of. */
function proves(proof: string, viewerCheck: string): boolean {
  const hashed = createHash("sha256")
    .update(Buffer.from(proof, "hex"))
    .digest();
  return timingSafeEqual(hashed, Buffer.from(viewerCheck, "hex"));
}

export interface ShownPage {
  storedAs: string;
  anyone: boolean;
}

/**
 * The live page a proof opens, or a 404 that never says whether the page is missing, the proof
 * wrong, or the share over.
 */
export async function shownPage(
  db: Kysely<Tables>,
  id: string,
  proof: string,
): Promise<ShownPage> {
  const row = await db
    .selectFrom("pages")
    .select(["stored_as", "viewer_check", "anyone"])
    .where("id", "=", id)
    .where("ended_at", "is", null)
    .where("expires_at", ">", new Date())
    .executeTakeFirst();
  if (!row?.stored_as || !proves(proof, row.viewer_check))
    throw notFound("No page is shared under that link.");
  return { storedAs: row.stored_as, anyone: row.anyone };
}

export async function countView(db: Kysely<Tables>, id: string): Promise<void> {
  await db
    .updateTable("pages")
    .set({ views: sql<number>`views + 1` })
    .where("id", "=", id)
    .execute();
}

/**
 * Deletes the sealed pages of expired shares, and the rows of shares that ended or expired more
 * than 30 days ago, so the host could read how each ended until then.
 */
export async function purgePages(
  db: Kysely<Tables>,
  dir: string | null,
  log: Logger,
): Promise<void> {
  const expired = await db
    .with("old", (qb) =>
      qb
        .selectFrom("pages")
        .select(["id", "stored_as"])
        .where("stored_as", "is not", null)
        .where("expires_at", "<=", new Date())
        .forUpdate(),
    )
    .updateTable("pages")
    .set({ stored_as: null })
    .from("old")
    .whereRef("pages.id", "=", "old.id")
    .returning("old.stored_as as stored")
    .execute();
  const files = expired.flatMap((row) => (row.stored ? [row.stored] : []));
  if (dir) await removeStashFiles(dir, files);
  if (files.length) log.info({ pages: files.length }, "deleted expired pages");
  const kept = sql<Date>`now() - make_interval(days => ${KEPT_AFTER_DAYS})`;
  const gone = await db
    .deleteFrom("pages")
    .where((eb) =>
      eb.or([eb("expires_at", "<", kept), eb("ended_at", "<", kept)]),
    )
    .executeTakeFirst();
  if (gone.numDeletedRows > 0n)
    log.info({ pages: Number(gone.numDeletedRows) }, "purged old shared pages");
}
