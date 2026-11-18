import type {
  OrgRole,
  OrgVaultCreate,
  OrgVaultEntry,
  OrgVaultEntryPut,
  OrgVaultKeysAdd,
  OrgVaultRotate,
  OrgWrappedKey,
} from "@sikemux/protocol";
import { sql, type Transaction } from "kysely";

import { audit, type AuditAction } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import { ApiFailure, badRequest, forbidden, notFound } from "../http.ts";
import {
  ENTRY_COLUMNS,
  memberHosts,
  PROJECT_KINDS,
  PROJECT_MEMORY,
  PROJECT_MEMORY_DROPS,
  PROJECT_MEMORY_WAITING,
  toEntry,
} from "./state.ts";

/** The most entries of each project kind an org's vault holds, so they never crowd out the rest. */
const MAX_ENTRIES: Record<string, number> = {
  [PROJECT_MEMORY]: 500,
  [PROJECT_MEMORY_WAITING]: 2000,
  [PROJECT_MEMORY_DROPS]: 500,
};
/** The most entries of every other kind together, such as model keys. */
const MAX_OTHER_ENTRIES = 500;

export interface Asker {
  orgId: string;
  userId: string;
  hostKey: string;
}

function conflict(message: string): ApiFailure {
  return new ApiFailure(409, "conflict", message);
}

/** The org's vault, locked until the transaction ends so its changes happen one at a time. */
async function lockedVault(trx: Transaction<Tables>, orgId: string) {
  const vault = await trx
    .selectFrom("org_vaults")
    .select("generation")
    .where("org_id", "=", orgId)
    .forUpdate()
    .executeTakeFirst();
  if (!vault) throw notFound("The org has no vault yet.");
  return vault;
}

function atCurrent(current: number, generation: number) {
  if (generation !== current)
    throw conflict(
      `The org's key is at generation ${current}, not ${generation}.`,
    );
}

async function requireHolder(
  trx: Transaction<Tables>,
  { orgId, hostKey }: Asker,
  generation: number,
) {
  const held = await trx
    .selectFrom("org_vault_keys")
    .select("host_key")
    .where("org_id", "=", orgId)
    .where("host_key", "=", hostKey)
    .where("generation", "=", generation)
    .executeTakeFirst();
  if (!held)
    throw forbidden(
      `This host does not hold the org's key at generation ${generation}.`,
    );
}

/** Refuses keys for anything but the members' hosts, or two for one host. */
async function requireMemberHosts(
  trx: Transaction<Tables>,
  orgId: string,
  keys: OrgWrappedKey[],
) {
  const wanted = new Set(keys.map((key) => key.hostKey));
  if (wanted.size !== keys.length)
    throw badRequest("Each host gets the key once.");
  const hosts = await memberHosts(trx, orgId)
    .where("devices.key", "in", [...wanted])
    .execute();
  if (hosts.length !== wanted.size)
    throw badRequest("A key is for a device that is not a member's host.");
}

function wrapRows(
  { orgId, hostKey }: Asker,
  generation: number,
  keys: OrgWrappedKey[],
) {
  return keys.map((key) => ({
    org_id: orgId,
    host_key: key.hostKey,
    generation,
    wrapped: key.wrapped,
    wrapped_by: hostKey,
  }));
}

/** Tells every member's hosts to read the org's vault again. */
export async function vaultChanged(trx: Transaction<Tables>, orgId: string) {
  const members = await trx
    .selectFrom("org_users")
    .select("user_id")
    .where("org_id", "=", orgId)
    .execute();
  await appendEvents(
    trx,
    members.map(({ user_id }) => ({ userId: user_id, type: "orgs.changed" })),
  );
}

/** Keeps the last revision of each removed entry, for the next entry under its id to start above. */
export async function keepEnded(
  trx: Transaction<Tables>,
  orgId: string,
  removed: { id: string; revision: number }[],
) {
  if (!removed.length) return;
  await trx
    .insertInto("org_vault_ended")
    .values(
      removed.map(({ id, revision }) => ({ org_id: orgId, id, revision })),
    )
    .onConflict((oc) =>
      oc.columns(["org_id", "id"]).doUpdateSet({
        revision: (eb) => eb.ref("excluded.revision"),
      }),
    )
    .execute();
}

function auditOrg(
  trx: Transaction<Tables>,
  { orgId, userId, hostKey }: Asker,
  action: AuditAction,
  detail: Record<string, unknown>,
) {
  return audit(trx, {
    userId,
    actor: `device:${hostKey}`,
    action,
    subject: orgId,
    detail: { org: orgId, ...detail },
  });
}

export async function createOrgVault(
  trx: Transaction<Tables>,
  asker: Asker,
  { key }: OrgVaultCreate,
): Promise<void> {
  const made = await trx
    .insertInto("org_vaults")
    .values({ org_id: asker.orgId, created_by: asker.hostKey })
    .onConflict((oc) => oc.column("org_id").doNothing())
    .returning("org_id")
    .executeTakeFirst();
  if (!made) throw conflict("The org already has a vault.");
  await trx
    .insertInto("org_vault_keys")
    .values(wrapRows(asker, 1, [{ hostKey: asker.hostKey, wrapped: key }]))
    .execute();
  await vaultChanged(trx, asker.orgId);
  await auditOrg(trx, asker, "org_vault.created", {});
}

/** Refuses keys for hosts of another account, which only an owner's or admin's host may seal for. */
async function requireOwnHosts(
  trx: Transaction<Tables>,
  { userId }: Asker,
  keys: OrgWrappedKey[],
) {
  const own = await trx
    .selectFrom("devices")
    .select("key")
    .where("user_id", "=", userId)
    .where(
      "key",
      "in",
      keys.map((key) => key.hostKey),
    )
    .execute();
  if (own.length !== keys.length)
    throw forbidden(
      "Only an org's owner or admin may give the vault's key to another account's host.",
    );
}

/**
 * A member's host holding the key seals it for other members' hosts: an owner's or admin's for
 * any, anyone else's only for its own account's. A host that has a wrap keeps it.
 */
export async function addOrgKeys(
  trx: Transaction<Tables>,
  asker: Asker,
  role: OrgRole,
  { generation, keys }: OrgVaultKeysAdd,
): Promise<void> {
  atCurrent((await lockedVault(trx, asker.orgId)).generation, generation);
  await requireHolder(trx, asker, generation);
  await requireMemberHosts(trx, asker.orgId, keys);
  if (role === "user") await requireOwnHosts(trx, asker, keys);
  const added = await trx
    .insertInto("org_vault_keys")
    .values(wrapRows(asker, generation, keys))
    .onConflict((oc) => oc.columns(["org_id", "host_key"]).doNothing())
    .returning("host_key")
    .execute();
  if (!added.length) return;
  await vaultChanged(trx, asker.orgId);
  const given = await trx
    .selectFrom("devices")
    .select(["key", "user_id", "name"])
    .where(
      "key",
      "in",
      added.map((row) => row.host_key),
    )
    .execute();
  for (const host of given)
    await auditOrg(trx, asker, "org_vault.key_added", {
      generation,
      host: host.key,
      member: host.user_id,
      computer: host.name,
    });
}

/** Every wrap is replaced by the new key's, so a host that left keeps only a key nothing is sealed under. */
export async function rotateOrgVault(
  trx: Transaction<Tables>,
  asker: Asker,
  { from, keys, entries }: OrgVaultRotate,
): Promise<void> {
  const { orgId, hostKey } = asker;
  if (!keys.some((key) => key.hostKey === hostKey))
    throw badRequest("The new key is sealed for this host too.");
  atCurrent((await lockedVault(trx, orgId)).generation, from);
  await requireHolder(trx, asker, from);
  await requireMemberHosts(trx, orgId, keys);
  const current = await trx
    .selectFrom("org_vault_entries")
    .select(["id", "revision"])
    .where("org_id", "=", orgId)
    .execute();
  const revisions = new Map(current.map((row) => [row.id, row.revision]));
  const named = new Set(entries.map((entry) => entry.id));
  if (
    named.size !== entries.length ||
    entries.length !== current.length ||
    entries.some((entry) => revisions.get(entry.id) !== entry.revision)
  )
    throw conflict(
      "The entries are not every entry in the vault at its current revision.",
    );
  const next = from + 1;
  await trx
    .updateTable("org_vaults")
    .set({ generation: next, rotated_at: sql<Date>`now()` })
    .where("org_id", "=", orgId)
    .execute();
  await trx.deleteFrom("org_vault_keys").where("org_id", "=", orgId).execute();
  await trx
    .insertInto("org_vault_keys")
    .values(wrapRows(asker, next, keys))
    .execute();
  if (entries.length)
    await sql`
      update org_vault_entries
      set sealed = resealed.sealed, generation = ${next}, revision = revision + 1,
        updated_by = ${hostKey}, updated_at = now()
      from unnest(${entries.map((entry) => entry.id)}::text[], ${entries.map((entry) => entry.sealed)}::text[])
        as resealed (id, sealed)
      where org_vault_entries.id = resealed.id and org_vault_entries.org_id = ${orgId}`.execute(
      trx,
    );
  await vaultChanged(trx, orgId);
  await auditOrg(trx, asker, "org_vault.rotated", {
    generation: next,
    hosts: keys.length,
    entries: entries.length,
  });
}

export async function putOrgEntry(
  trx: Transaction<Tables>,
  asker: Asker,
  id: string,
  { kind, generation, sealed, revision }: OrgVaultEntryPut,
): Promise<OrgVaultEntry> {
  const { orgId, hostKey } = asker;
  atCurrent((await lockedVault(trx, orgId)).generation, generation);
  await requireHolder(trx, asker, generation);
  const existing = await trx
    .selectFrom("org_vault_entries")
    .select("revision")
    .where("org_id", "=", orgId)
    .where("id", "=", id)
    .executeTakeFirst();
  const ended = existing
    ? undefined
    : await trx
        .selectFrom("org_vault_ended")
        .select("revision")
        .where("org_id", "=", orgId)
        .where("id", "=", id)
        .executeTakeFirst();
  const floor = ended?.revision ?? 0;
  if ((existing?.revision ?? floor) !== revision)
    throw conflict(
      existing
        ? `The entry is at revision ${existing.revision}, not ${revision}.`
        : `The vault has no such entry; add it at revision ${floor}.`,
    );
  let row;
  if (existing) {
    row = await trx
      .updateTable("org_vault_entries")
      .set({
        kind,
        generation,
        sealed,
        revision: revision + 1,
        updated_by: hostKey,
        updated_at: sql<Date>`now()`,
      })
      .where("org_id", "=", orgId)
      .where("id", "=", id)
      .returning(ENTRY_COLUMNS)
      .executeTakeFirstOrThrow();
  } else {
    const cap = MAX_ENTRIES[kind];
    const held = await trx
      .selectFrom("org_vault_entries")
      .select((eb) => eb.fn.countAll<string>().as("count"))
      .where("org_id", "=", orgId)
      .$if(cap !== undefined, (qb) => qb.where("kind", "=", kind))
      .$if(cap === undefined, (qb) => qb.where("kind", "not in", PROJECT_KINDS))
      .executeTakeFirstOrThrow();
    const most = cap ?? MAX_OTHER_ENTRIES;
    if (Number(held.count) >= most)
      throw conflict(
        `An org's vault holds at most ${most} entries of that kind.`,
      );
    row = await trx
      .insertInto("org_vault_entries")
      .values({
        id,
        org_id: orgId,
        kind,
        generation,
        sealed,
        revision: revision + 1,
        updated_by: hostKey,
      })
      .returning(ENTRY_COLUMNS)
      .executeTakeFirstOrThrow();
    if (ended)
      await trx
        .deleteFrom("org_vault_ended")
        .where("org_id", "=", orgId)
        .where("id", "=", id)
        .execute();
  }
  await vaultChanged(trx, orgId);
  await auditOrg(trx, asker, "org_vault.entry_written", {
    entry: id,
    kind,
    generation,
    revision: row.revision,
  });
  return toEntry(row);
}

export async function removeOrgEntry(
  trx: Transaction<Tables>,
  asker: Asker,
  id: string,
  revision: number,
): Promise<void> {
  const { orgId } = asker;
  await lockedVault(trx, orgId);
  const removed = await trx
    .deleteFrom("org_vault_entries")
    .where("id", "=", id)
    .where("org_id", "=", orgId)
    .where("revision", "=", revision)
    .returning(["id", "kind", "revision"])
    .executeTakeFirst();
  if (!removed) {
    const existing = await trx
      .selectFrom("org_vault_entries")
      .select("revision")
      .where("id", "=", id)
      .where("org_id", "=", orgId)
      .executeTakeFirst();
    if (!existing) throw notFound("No entry with that id is in the vault.");
    throw conflict(
      `The entry is at revision ${existing.revision}, not ${revision}.`,
    );
  }
  await keepEnded(trx, orgId, [removed]);
  await vaultChanged(trx, orgId);
  await auditOrg(trx, asker, "org_vault.entry_removed", {
    entry: id,
    kind: removed.kind,
    revision,
  });
}
