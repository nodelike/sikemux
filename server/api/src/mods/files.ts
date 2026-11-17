import { randomBytes } from "node:crypto";
import type { ModFileStored, ModFiles } from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { ApiFailure, notFound } from "../http.ts";
import type { Logger } from "../log.ts";
import type { StashFiles } from "../stash/files.ts";
import { removeStashFiles } from "../stash/files.ts";
import { lockedCounter, type Asker } from "./change.ts";

/** A picture or other file of a mod, sealed by the host, which adds 28 bytes to it. */
export const MAX_MOD_FILE_BYTES = 25 * 1024 * 1024 + 64;
export const MAX_MOD_FILES_BYTES = 256 * 1024 * 1024;

export interface ModFile {
  mod: string;
  file: string;
}

function tooLarge(message: string): ApiFailure {
  return new ApiFailure(413, "payload_too_large", message);
}

async function storedAs(
  db: Kysely<Tables>,
  userId: string,
  { mod, file }: ModFile,
): Promise<string | undefined> {
  const row = await db
    .selectFrom("mod_files")
    .select("stored_as")
    .where("user_id", "=", userId)
    .where("mod_id", "=", mod)
    .where("file_id", "=", file)
    .executeTakeFirst();
  return row?.stored_as;
}

export async function listFiles(
  db: Kysely<Tables>,
  userId: string,
  mod: string,
): Promise<ModFiles> {
  const rows = await db
    .selectFrom("mod_files")
    .select("file_id")
    .where("user_id", "=", userId)
    .where("mod_id", "=", mod)
    .execute();
  return { files: rows.map((row) => row.file_id) };
}

/**
 * Keeps a mod's file in the stash's encrypted file store under a name of its own, so the stash's
 * items and space are untouched. A file the account holds already is not read again, since its
 * id names its content.
 */
export async function putFile(
  db: Kysely<Tables>,
  store: StashFiles,
  { userId, hostKey }: Asker,
  file: ModFile,
  declared: number,
  body: AsyncIterable<Uint8Array>,
  checkBody: (sha256: string) => void,
): Promise<ModFileStored> {
  if (declared > MAX_MOD_FILE_BYTES)
    throw tooLarge(`A mod's file is limited to ${MAX_MOD_FILE_BYTES} bytes.`);
  if (await storedAs(db, userId, file)) return { stored: false };
  const name = randomBytes(16).toString("base64url");
  const written = await store.write(name, body, MAX_MOD_FILE_BYTES);
  try {
    checkBody(written.sha256);
    const stored = await db.transaction().execute(async (trx) => {
      await lockedCounter(trx, userId);
      const { file_bytes } = await trx
        .selectFrom("mod_counters")
        .select("file_bytes")
        .where("user_id", "=", userId)
        .executeTakeFirstOrThrow();
      if (Number(file_bytes) + written.size > MAX_MOD_FILES_BYTES)
        throw tooLarge(
          `An account holds at most ${MAX_MOD_FILES_BYTES} bytes of mod files. Remove a mod to make room.`,
        );
      const inserted = await trx
        .insertInto("mod_files")
        .values({
          user_id: userId,
          mod_id: file.mod,
          file_id: file.file,
          stored_as: name,
          size: written.size,
          stored_by: hostKey,
        })
        .onConflict((oc) =>
          oc.columns(["user_id", "mod_id", "file_id"]).doNothing(),
        )
        .returning("file_id")
        .executeTakeFirst();
      if (!inserted) return false;
      await trx
        .updateTable("mod_counters")
        .set({ file_bytes: sql<number>`file_bytes + ${written.size}` })
        .where("user_id", "=", userId)
        .execute();
      await appendEvent(trx, {
        userId,
        type: "mods.changed",
        subject: hostKey,
      });
      await store.commit(name);
      return true;
    });
    if (!stored) await store.discard(name);
    return { stored };
  } catch (error) {
    await store.discard(name);
    throw error;
  }
}

export async function readFile(
  db: Kysely<Tables>,
  store: StashFiles,
  userId: string,
  file: ModFile,
  log: Logger,
): Promise<ReadableStream> {
  const name = await storedAs(db, userId, file);
  const stream = name ? await store.read(name, log) : null;
  if (!stream) throw notFound("The account holds no such file of that mod.");
  return stream;
}

export async function removeFile(
  db: Kysely<Tables>,
  store: StashFiles | null,
  userId: string,
  { mod, file }: ModFile,
): Promise<void> {
  const removed = await db.transaction().execute(async (trx) => {
    await lockedCounter(trx, userId);
    const row = await trx
      .deleteFrom("mod_files")
      .where("user_id", "=", userId)
      .where("mod_id", "=", mod)
      .where("file_id", "=", file)
      .returning(["stored_as", "size"])
      .executeTakeFirst();
    if (row)
      await trx
        .updateTable("mod_counters")
        .set({ file_bytes: sql<number>`file_bytes - ${Number(row.size)}` })
        .where("user_id", "=", userId)
        .execute();
    return row?.stored_as;
  });
  if (removed && store) await removeStashFiles(store.dir, [removed]);
}
