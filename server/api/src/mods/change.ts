import type {
  ModConflict,
  ModDocumentPut,
  ModStored,
  ModsPush,
  ModsPushed,
  ModsPurged,
} from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { ApiFailure, badRequest, notFound } from "../http.ts";
import { DOCUMENT_COLUMNS, toDocument } from "./documents.ts";

export const MAX_SEALED_BYTES = 64 * 1024 * 1024;

interface Asker {
  userId: string;
  hostKey: string;
}

/**
 * The account's mod counter, locked until the transaction ends. Every change to the account's
 * mod documents takes it first, so seqs commit in order and a pull never skips one.
 */
async function lockedCounter(trx: Transaction<Tables>, userId: string) {
  const vault = await trx
    .selectFrom("vaults")
    .select("generation")
    .where("user_id", "=", userId)
    .forShare()
    .executeTakeFirst();
  if (!vault) throw notFound("The account has no vault.");
  await trx
    .insertInto("mod_counters")
    .values({ user_id: userId })
    .onConflict((oc) => oc.column("user_id").doNothing())
    .execute();
  const counter = await trx
    .selectFrom("mod_counters")
    .select(["seq", "sealed_bytes"])
    .where("user_id", "=", userId)
    .forUpdate()
    .executeTakeFirstOrThrow();
  return {
    generation: vault.generation,
    seq: Number(counter.seq),
    sealedBytes: Number(counter.sealed_bytes),
  };
}

async function requireHolder(
  trx: Transaction<Tables>,
  { userId, hostKey }: Asker,
  generation: number,
) {
  const held = await trx
    .selectFrom("vault_keys")
    .select("host_key")
    .where("user_id", "=", userId)
    .where("host_key", "=", hostKey)
    .where("generation", "=", generation)
    .executeTakeFirst();
  if (!held)
    throw new ApiFailure(
      403,
      "forbidden",
      `This host does not hold the vault key at generation ${generation}.`,
    );
}

const keyOf = (document: { mod: string; id: string }) =>
  `${document.mod}/${document.id}`;

export async function pushDocuments(
  db: Kysely<Tables>,
  asker: Asker,
  { generation, documents }: ModsPush,
): Promise<ModsPushed> {
  const { userId, hostKey } = asker;
  if (new Set(documents.map(keyOf)).size !== documents.length)
    throw badRequest("Each document is pushed once.");
  return db.transaction().execute(async (trx) => {
    const counter = await lockedCounter(trx, userId);
    if (counter.generation !== generation)
      throw new ApiFailure(
        409,
        "conflict",
        `The vault's key is at generation ${counter.generation}, not ${generation}.`,
      );
    await requireHolder(trx, asker, generation);
    const existing = await trx
      .selectFrom("mod_documents")
      .select(DOCUMENT_COLUMNS)
      .where("user_id", "=", userId)
      .where(({ eb, refTuple, tuple }) =>
        eb(
          refTuple("mod_id", "doc_id"),
          "in",
          documents.map((document) => tuple(document.mod, document.id)),
        ),
      )
      .execute();
    const current = new Map(
      existing.map((row) => [`${row.mod_id}/${row.doc_id}`, row]),
    );

    const conflicts: ModConflict[] = [];
    const accepted: ModDocumentPut[] = [];
    let sealedBytes = counter.sealedBytes;
    for (const document of documents) {
      const row = current.get(keyOf(document));
      if ((row?.revision ?? 0) !== document.revision) {
        conflicts.push({
          mod: document.mod,
          id: document.id,
          current: row ? toDocument(row) : null,
        });
        continue;
      }
      accepted.push(document);
      sealedBytes += document.sealed.length - (row?.sealed.length ?? 0);
    }
    if (sealedBytes > MAX_SEALED_BYTES && sealedBytes > counter.sealedBytes)
      throw new ApiFailure(
        413,
        "payload_too_large",
        `An account holds at most ${MAX_SEALED_BYTES} bytes of sealed mod documents; this push would make it ${sealedBytes}.`,
      );
    if (accepted.length === 0)
      return { seq: counter.seq, stored: [], conflicts };

    const stored: ModStored[] = accepted.map((document, index) => ({
      mod: document.mod,
      id: document.id,
      revision: document.revision + 1,
      seq: counter.seq + index + 1,
    }));
    const seq = counter.seq + accepted.length;
    await trx
      .insertInto("mod_documents")
      .values(
        accepted.map((document, index) => ({
          user_id: userId,
          mod_id: document.mod,
          doc_id: document.id,
          generation,
          sealed: document.sealed,
          removed: document.removed,
          revision: document.revision + 1,
          seq: counter.seq + index + 1,
          updated_by: hostKey,
        })),
      )
      .onConflict((oc) =>
        oc.columns(["user_id", "mod_id", "doc_id"]).doUpdateSet((eb) => ({
          generation: eb.ref("excluded.generation"),
          sealed: eb.ref("excluded.sealed"),
          removed: eb.ref("excluded.removed"),
          revision: eb.ref("excluded.revision"),
          seq: eb.ref("excluded.seq"),
          updated_by: eb.ref("excluded.updated_by"),
          updated_at: sql<Date>`now()`,
        })),
      )
      .execute();
    await trx
      .updateTable("mod_counters")
      .set({ seq, sealed_bytes: sealedBytes })
      .where("user_id", "=", userId)
      .execute();
    await appendEvent(trx, {
      userId,
      type: "mods.changed",
      subject: hostKey,
    });
    return { seq, stored, conflicts };
  });
}

export async function purgeMod(
  db: Kysely<Tables>,
  { userId, hostKey }: Asker,
  mod: string,
): Promise<ModsPurged> {
  return db.transaction().execute(async (trx) => {
    const counter = await lockedCounter(trx, userId);
    const removed = await trx
      .deleteFrom("mod_documents")
      .where("user_id", "=", userId)
      .where("mod_id", "=", mod)
      .returning((eb) => eb.fn("length", ["sealed"]).as("length"))
      .execute();
    const freed = removed.reduce((sum, row) => sum + Number(row.length), 0);
    const seq = counter.seq + 1;
    await trx
      .insertInto("mod_purges")
      .values({ user_id: userId, seq, mod_id: mod, purged_by: hostKey })
      .execute();
    await trx
      .updateTable("mod_counters")
      .set({ seq, sealed_bytes: counter.sealedBytes - freed })
      .where("user_id", "=", userId)
      .execute();
    await appendEvent(trx, {
      userId,
      type: "mods.changed",
      subject: hostKey,
    });
    await audit(trx, {
      userId,
      actor: `device:${hostKey}`,
      action: "mods.purged",
      subject: mod,
      detail: { documents: removed.length, seq },
    });
    return { seq };
  });
}
