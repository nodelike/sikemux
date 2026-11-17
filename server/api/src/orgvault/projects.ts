import { sql, type RawBuilder, type Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { forbidden } from "../http.ts";
import type { Member } from "./state.ts";
import { vaultChanged } from "./change.ts";

/** A shared project's store of rules and knowledge, which agents read. */
export const PROJECT_MEMORY = "project-memory";
/** Rules members added to a shared project, waiting for an owner or admin to keep or drop them. */
export const PROJECT_MEMORY_WAITING = "project-memory-waiting";
const PROJECT_KINDS = [PROJECT_MEMORY, PROJECT_MEMORY_WAITING];

/** The entry id of a project's `kind`: the first 32 hex of SHA-256 over `<kind>|<project key>`. */
function entryIdOf(
  kind: string | RawBuilder<unknown>,
  project: RawBuilder<unknown>,
) {
  return sql<string>`substr(encode(sha256(convert_to(${kind} || '|' || ${project}, 'UTF8')), 'hex'), 1, 32)`;
}

/**
 * Who may put an entry of `kind`. A shared project's entries go only at their own id while some
 * member shares the project with the org. Any member writes the waiting list, and the store too
 * while the org lets every member add rules; other kinds are the owners' and admins'.
 */
export async function allowPut(
  trx: Transaction<Tables>,
  orgId: string,
  member: Member,
  id: string,
  kind: string,
) {
  if (!PROJECT_KINDS.includes(kind)) {
    if (member.role === "user")
      throw forbidden("Only an org's owner or admin may change its vault.");
    return;
  }
  const shared = await trx
    .selectFrom("org_shares")
    .innerJoin("org_users", "org_users.id", "org_shares.member_id")
    .select("org_shares.project")
    .where("org_users.org_id", "=", orgId)
    .where(entryIdOf(kind, sql.ref("org_shares.project")), "=", id)
    .executeTakeFirst();
  if (!shared)
    throw forbidden("No member shares a project with the org at that id.");
  if (member.role !== "user" || kind === PROJECT_MEMORY_WAITING) return;
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

/** Removes the entries of projects no member shares with the org any more. */
export async function dropUnshared(trx: Transaction<Tables>, orgId: string) {
  const dropped = await trx
    .deleteFrom("org_vault_entries")
    .where("org_id", "=", orgId)
    .where("kind", "in", PROJECT_KINDS)
    .where(({ not, exists, selectFrom }) =>
      not(
        exists(
          selectFrom("org_shares")
            .innerJoin("org_users", "org_users.id", "org_shares.member_id")
            .select(sql`1`.as("one"))
            .where("org_users.org_id", "=", orgId)
            .whereRef(
              entryIdOf(
                sql.ref("org_vault_entries.kind"),
                sql.ref("org_shares.project"),
              ),
              "=",
              "org_vault_entries.id",
            ),
        ),
      ),
    )
    .executeTakeFirst();
  if (dropped.numDeletedRows > 0n) await vaultChanged(trx, orgId);
}
