import { sql, type RawBuilder, type Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { forbidden } from "../http.ts";
import {
  PROJECT_KINDS,
  PROJECT_MEMORY,
  PROJECT_MEMORY_DROPS,
  PROJECT_MEMORY_WAITING,
  type Member,
} from "./state.ts";
import { keepEnded, vaultChanged } from "./change.ts";

/**
 * The entry id of a project's `kind`: the first 32 hex of SHA-256 over `<kind>|<project key>`,
 * or for a member's waiting rules over `<kind>|<project key>|<membership>`.
 */
function entryIdOf(
  kind: string | RawBuilder<unknown>,
  project: RawBuilder<unknown>,
  membership: string | RawBuilder<unknown>,
) {
  const of = sql`case when ${kind}::text = ${PROJECT_MEMORY_WAITING}::text then ${project} || '|' || ${membership}::text else ${project} end`;
  return sql<string>`substr(encode(sha256(convert_to(${kind} || '|' || ${of}, 'UTF8')), 'hex'), 1, 32)`;
}

/**
 * Who may put an entry of `kind`. A shared project's entries go only at their own id while some
 * member shares the project with the org. A member's waiting rules are theirs alone to write, the
 * drops are the owners' and admins', and the store is too unless the org lets every member add
 * rules; other kinds are the owners' and admins'.
 */
export async function allowPut(
  trx: Transaction<Tables>,
  orgId: string,
  member: Member,
  id: string,
  kind: string,
) {
  if (!PROJECT_KINDS.includes(kind) || kind === PROJECT_MEMORY_DROPS) {
    if (member.role === "user")
      throw forbidden("Only an org's owner or admin may change its vault.");
    if (kind !== PROJECT_MEMORY_DROPS) return;
  }
  const shared = await trx
    .selectFrom("org_shares")
    .innerJoin("org_users", "org_users.id", "org_shares.member_id")
    .select("org_shares.project")
    .where("org_users.org_id", "=", orgId)
    .where(entryIdOf(kind, sql.ref("org_shares.project"), member.id), "=", id)
    .executeTakeFirst();
  if (!shared)
    throw forbidden("No member shares a project with the org at that id.");
  if (member.role !== "user" || kind !== PROJECT_MEMORY) return;
  const org = await trx
    .selectFrom("orgs")
    .select("rule_adding")
    .where("id", "=", orgId)
    .executeTakeFirstOrThrow();
  if (org.rule_adding !== "every_member")
    throw forbidden(
      "The org's owners and admins approve rules first: add it to the waiting list.",
    );
}

/**
 * Removes the entries of projects no member shares with the org any more, and the waiting rules
 * of members who left or no longer share that project themselves.
 */
export async function dropUnshared(trx: Transaction<Tables>, orgId: string) {
  const vault = await trx
    .selectFrom("org_vaults")
    .select("org_id")
    .where("org_id", "=", orgId)
    .forUpdate()
    .executeTakeFirst();
  if (!vault) return;
  const dropped = await trx
    .deleteFrom("org_vault_entries")
    .where("org_id", "=", orgId)
    .where("kind", "in", PROJECT_KINDS)
    .where(({ not, exists, selectFrom }) =>
      not(
        exists(
          selectFrom("org_shares")
            .innerJoin("org_users", "org_users.id", "org_shares.member_id")
            .innerJoin(
              "org_users as writer",
              "writer.org_id",
              "org_users.org_id",
            )
            .select(sql`1`.as("one"))
            .where("org_users.org_id", "=", orgId)
            .where((eb) =>
              eb.or([
                eb("org_vault_entries.kind", "<>", PROJECT_MEMORY_WAITING),
                eb("org_users.id", "=", eb.ref("writer.id")),
              ]),
            )
            .whereRef(
              entryIdOf(
                sql.ref("org_vault_entries.kind"),
                sql.ref("org_shares.project"),
                sql.ref("writer.id"),
              ),
              "=",
              "org_vault_entries.id",
            ),
        ),
      ),
    )
    .returning(["id", "revision"])
    .execute();
  await keepEnded(trx, orgId, dropped);
  if (dropped.length) await vaultChanged(trx, orgId);
}
