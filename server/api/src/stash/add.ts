import { randomBytes } from "node:crypto";
import type { StashAdd, StashTarget } from "@sikemux/protocol";
import { MAX_STASH_FILE_BYTES } from "@sikemux/protocol/stash";
import type { Kysely } from "kysely";

import type { Identity } from "../auth.ts";
import type { Tables } from "../db.ts";
import { ApiFailure, badRequest } from "../http.ts";
import type { Logger } from "../log.ts";
import type { StashFiles } from "./files.ts";
import {
  announce,
  checkSpace,
  checkTarget,
  holdAccount,
  type ItemRow,
} from "./items.ts";
import { isFetchable, linksIn, readLinks, readPages } from "./preview.ts";

/** A file to stash, as its upload's headers describe it. */
export type Upload = {
  kind: "image" | "video" | "file";
  mime: string;
  name: string;
  note: string | null;
  target: StashTarget | undefined;
  prompt: string | undefined;
  /** The size the upload says it has, before a byte is read. */
  declared: number;
};

function newId(): string {
  return randomBytes(16).toString("base64url");
}

/** The device the request came from: the one that signed it, or the phone a session belongs to. */
async function fromDevice(
  db: Kysely<Tables>,
  { userId, deviceKey, sessionId }: Identity,
): Promise<string | null> {
  if (deviceKey) return deviceKey;
  if (!sessionId) return null;
  const phone = await db
    .selectFrom("devices")
    .select("key")
    .where("user_id", "=", userId)
    .where("clerk_session_id", "=", sessionId)
    .executeTakeFirst();
  return phone?.key ?? null;
}

/** The account's prompt that is still waiting for attachments, locked while one is added. */
async function openPrompt(db: Kysely<Tables>, userId: string, id: string) {
  const row = await db
    .selectFrom("stash_items")
    .select("id")
    .where("id", "=", id)
    .where("user_id", "=", userId)
    .where("kind", "=", "prompt")
    .where("attachments_missing", ">", 0)
    .forUpdate()
    .executeTakeFirst();
  if (!row)
    throw badRequest("That prompt is not waiting for any more attachments.");
}

/** Saves what was read of an item's pages, once they are read, and tells its account. */
function saveRead(
  db: Kysely<Tables>,
  log: Logger,
  row: ItemRow,
  reading: Promise<{ preview: string } | { links: string } | undefined>,
) {
  reading
    .then(async (found) => {
      if (!found) return;
      await db.transaction().execute(async (trx) => {
        const updated = await trx
          .updateTable("stash_items")
          .set(found)
          .where("id", "=", row.id)
          .executeTakeFirst();
        if (updated.numUpdatedRows > 0n)
          await announce(trx, row.user_id, "stash.changed", row.id);
      });
    })
    .catch((error: unknown) =>
      log.warn({ err: error, id: row.id }, "could not save a link's preview"),
    );
}

/** Stashes a link, text or prompt, then reads the pages it points to behind the answer. */
export async function addItem(
  db: Kysely<Tables>,
  log: Logger,
  identity: Identity,
  request: StashAdd,
): Promise<ItemRow> {
  const { userId } = identity;
  const url = request.kind === "link" ? request.url : null;
  if (url !== null) {
    const parsed = URL.parse(url);
    if (!parsed || !["http:", "https:"].includes(parsed.protocol))
      throw badRequest("A link is an http or https url.");
  }
  const text = request.kind === "link" ? null : request.text;
  const missing =
    request.kind === "prompt" ? (request.attachmentCount ?? 0) : 0;
  const links = linksIn(text ?? "");
  if (request.target) await checkTarget(db, userId, request.target);
  const id = newId();
  const row = await db.transaction().execute(async (trx) => {
    await holdAccount(trx, userId);
    const row = await trx
      .insertInto("stash_items")
      .values({
        id,
        user_id: userId,
        kind: request.kind,
        url,
        text,
        note: request.note || null,
        name: null,
        mime: null,
        size: null,
        preview: null,
        links: links.length
          ? JSON.stringify(links.map((url) => ({ url, preview: null })))
          : null,
        target: request.target ? JSON.stringify(request.target) : null,
        from_device: await fromDevice(trx, identity),
        attachments_missing: missing,
      })
      .returningAll()
      .executeTakeFirstOrThrow();
    if (missing === 0) await announce(trx, userId, "stash.added", id);
    return row;
  });
  if (row.url) {
    const url = URL.parse(row.url);
    if (url && isFetchable(url))
      saveRead(
        db,
        log,
        row,
        readPages([row.url]).then((found) =>
          found?.[0] ? { preview: JSON.stringify(found[0]) } : undefined,
        ),
      );
  }
  if (links.length)
    saveRead(
      db,
      log,
      row,
      readLinks(links).then((read) =>
        read?.some((link) => link.preview)
          ? { links: JSON.stringify(read) }
          : undefined,
      ),
    );
  return row;
}

/**
 * Stashes a file from its `body`, written encrypted before its row is kept; `checkBody` refuses a
 * body whose hash is not the one its signature covered. A file attached to a prompt shows the
 * prompt once it is the last one.
 */
export async function addFile(
  db: Kysely<Tables>,
  store: StashFiles,
  identity: Identity,
  upload: Upload,
  body: AsyncIterable<Uint8Array>,
  checkBody: (sha256: string) => void,
): Promise<ItemRow> {
  const { userId } = identity;
  const { kind, mime, name, note, target, prompt } = upload;
  if (upload.declared > MAX_STASH_FILE_BYTES)
    throw new ApiFailure(
      413,
      "payload_too_large",
      `A stashed file is limited to ${MAX_STASH_FILE_BYTES} bytes.`,
    );
  await checkSpace(db, userId, upload.declared);
  if (target) await checkTarget(db, userId, target);
  if (prompt) await openPrompt(db, userId, prompt);
  const id = newId();
  const written = await store.write(id, body, MAX_STASH_FILE_BYTES);
  try {
    checkBody(written.sha256);
    const device = await fromDevice(db, identity);
    return await db.transaction().execute(async (trx) => {
      await holdAccount(trx, userId);
      await checkSpace(trx, userId, written.size);
      if (prompt) await openPrompt(trx, userId, prompt);
      const row = await trx
        .insertInto("stash_items")
        .values({
          id,
          user_id: userId,
          kind,
          url: null,
          text: null,
          note,
          name,
          mime,
          size: written.size,
          preview: null,
          target: target ? JSON.stringify(target) : null,
          from_device: device,
          prompt_id: prompt ?? null,
        })
        .returningAll()
        .executeTakeFirstOrThrow();
      if (prompt) {
        const { attachments_missing } = await trx
          .updateTable("stash_items")
          .set((eb) => ({
            attachments_missing: eb("attachments_missing", "-", 1),
          }))
          .where("id", "=", prompt)
          .returning("attachments_missing")
          .executeTakeFirstOrThrow();
        if (attachments_missing === 0)
          await announce(trx, userId, "stash.added", prompt);
      } else await announce(trx, userId, "stash.added", id);
      await store.commit(id);
      return row;
    });
  } catch (error) {
    await store.discard(id);
    throw error;
  }
}
