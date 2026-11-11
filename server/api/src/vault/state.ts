import type {
  Vault,
  VaultEntry,
  VaultState,
  VaultUses,
} from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";

type EntryRow = Pick<
  Tables["vault_entries"],
  "id" | "plugin" | "generation" | "sealed" | "revision" | "updated_by"
> & { updated_at: Date };

export const ENTRY_COLUMNS = [
  "id",
  "plugin",
  "generation",
  "sealed",
  "revision",
  "updated_by",
  "updated_at",
] as const;

export function toEntry(row: EntryRow): VaultEntry {
  return {
    id: row.id,
    plugin: row.plugin,
    generation: row.generation,
    sealed: row.sealed,
    revision: row.revision,
    updatedBy: row.updated_by,
    updatedAt: row.updated_at.toISOString(),
  };
}

/** The account's vault as `hostKey` sees it: the key sealed for it, and who else holds it or waits. */
export async function vaultState(
  db: Kysely<Tables>,
  userId: string,
  hostKey: string,
): Promise<VaultState> {
  const vault = await db
    .selectFrom("vaults")
    .select(["generation", "recovery_key", "recovery_wrap", "rotate"])
    .where("user_id", "=", userId)
    .executeTakeFirst();
  if (!vault) return { vault: null };
  const [keys, hosts, entries] = await Promise.all([
    db
      .selectFrom("vault_keys")
      .select(["host_key", "wrapped"])
      .where("user_id", "=", userId)
      .where("generation", "=", vault.generation)
      .orderBy("created_at")
      .orderBy("host_key")
      .execute(),
    db
      .selectFrom("devices")
      .select("key")
      .where("user_id", "=", userId)
      .where("role", "=", "host")
      .orderBy("created_at")
      .orderBy("key")
      .execute(),
    db
      .selectFrom("vault_entries")
      .select(ENTRY_COLUMNS)
      .where("user_id", "=", userId)
      .orderBy("created_at")
      .orderBy("id")
      .execute(),
  ]);
  const holders = new Set(keys.map((key) => key.host_key));
  const state: Vault = {
    generation: vault.generation,
    recoveryKey: vault.recovery_key,
    recoveryWrap: vault.recovery_wrap,
    key: keys.find((key) => key.host_key === hostKey)?.wrapped ?? null,
    holders: [...holders],
    waiting: hosts.map((host) => host.key).filter((key) => !holders.has(key)),
    rotate: vault.rotate,
    entries: entries.map(toEntry),
  };
  return { vault: state };
}

/** Uses of the account's entries, or of the one entry asked for, in the last 30 days, most recent first. */
export async function vaultUses(
  db: Kysely<Tables>,
  userId: string,
  entry?: string,
): Promise<VaultUses> {
  let query = db
    .selectFrom("vault_uses")
    .leftJoin("devices", (join) =>
      join
        .onRef("devices.key", "=", "vault_uses.host_key")
        .onRef("devices.user_id", "=", "vault_uses.user_id"),
    )
    .select([
      "vault_uses.entry_id",
      "vault_uses.plugin",
      "vault_uses.host_key",
      "devices.name",
      "vault_uses.actor",
      "vault_uses.count",
      "vault_uses.first_at",
      "vault_uses.last_at",
    ])
    .where("vault_uses.user_id", "=", userId)
    .where("vault_uses.last_at", ">", sql<Date>`now() - interval '30 days'`);
  if (entry !== undefined)
    query = query.where("vault_uses.entry_id", "=", entry);
  const rows = await query
    .orderBy("vault_uses.last_at", "desc")
    .orderBy("vault_uses.id", "desc")
    .limit(500)
    .execute();
  return {
    uses: rows.map((row) => ({
      entryId: row.entry_id,
      plugin: row.plugin,
      hostKey: row.host_key,
      hostName: row.name,
      actor: row.actor,
      count: row.count,
      firstAt: row.first_at.toISOString(),
      lastAt: row.last_at.toISOString(),
    })),
  };
}
