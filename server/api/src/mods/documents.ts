import type { ModDocument, ModsPulled } from "@sikemux/protocol";
import { sql, type Kysely, type Selectable } from "kysely";

import type { Tables } from "../db.ts";

const PAGE_DOCUMENTS = 500;
const PAGE_SEALED_BYTES = 4 * 1024 * 1024;

export const DOCUMENT_COLUMNS = [
  "mod_id",
  "doc_id",
  "generation",
  "sealed",
  "removed",
  "revision",
  "seq",
  "updated_by",
  "updated_at",
] as const;

type DocumentRow = Pick<
  Selectable<Tables["mod_documents"]>,
  (typeof DOCUMENT_COLUMNS)[number]
>;

export function toDocument(row: DocumentRow): ModDocument {
  return {
    mod: row.mod_id,
    id: row.doc_id,
    generation: row.generation,
    sealed: row.sealed,
    removed: row.removed,
    revision: row.revision,
    seq: Number(row.seq),
    updatedBy: row.updated_by,
    updatedAt: row.updated_at.toISOString(),
  };
}

/**
 * The account's mod documents and purges after `since`, oldest first, a page at a time. A cursor
 * older than removals the server has since forgotten, or newer than any it gave, starts over at 0.
 */
export async function pullDocuments(
  db: Kysely<Tables>,
  userId: string,
  since: number,
): Promise<ModsPulled> {
  const counter = await db
    .selectFrom("mod_counters")
    .select(["seq", "cleared_seq"])
    .where("user_id", "=", userId)
    .executeTakeFirst();
  const latest = Number(counter?.seq ?? 0);
  const reset =
    since > latest || (since > 0 && since < Number(counter?.cleared_seq ?? 0));
  const after = reset ? 0 : since;

  const { rows } = await sql<DocumentRow & { read: string }>`
    select ${sql.join(DOCUMENT_COLUMNS.map((column) => sql.ref(column)))}, read
    from (
      select *,
        sum(length(sealed)) over (order by seq) - length(sealed) as before,
        count(*) over () as read
      from (
        select * from mod_documents
        where user_id = ${userId} and seq > ${after} and seq <= ${latest}
        order by seq
        limit ${PAGE_DOCUMENTS}
      ) as first
    ) as page
    where before < ${PAGE_SEALED_BYTES}
    order by seq`.execute(db);
  const read = Number(rows[0]?.read ?? 0);
  const more = rows.length < read || read === PAGE_DOCUMENTS;
  const last = rows.at(-1);
  const next = more && last ? Number(last.seq) : latest;
  const purges = await db
    .selectFrom("mod_purges")
    .select(["mod_id", "seq"])
    .where("user_id", "=", userId)
    .where("seq", ">", String(after))
    .where("seq", "<=", String(next))
    .orderBy("seq")
    .execute();
  return {
    documents: rows.map(toDocument),
    purges: purges.map((row) => ({ mod: row.mod_id, seq: Number(row.seq) })),
    next,
    more,
    reset,
  };
}
