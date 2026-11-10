import type {
  VaultCreate,
  VaultEntry,
  VaultEntryPut,
  VaultKeysAdd,
  VaultRotate,
  VaultUsesAdd,
  WrappedKey,
} from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import { audit } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";
import { ApiFailure, badRequest, notFound } from "../http.ts";
import { ENTRY_COLUMNS, toEntry } from "./state.ts";

const MAX_ENTRIES = 500;

interface Asker {
  userId: string;
  hostKey: string;
}

function conflict(message: string): ApiFailure {
  return new ApiFailure(409, "conflict", message);
}

/** The account's vault, locked until the transaction ends so its changes happen one at a time. */
async function lockedVault(trx: Transaction<Tables>, userId: string) {
  const vault = await trx
    .selectFrom("vaults")
    .select("generation")
    .where("user_id", "=", userId)
    .forUpdate()
    .executeTakeFirst();
  if (!vault) throw notFound("The account has no vault.");
  return vault;
}

function atCurrent(current: number, generation: number) {
  if (generation !== current)
    throw conflict(
      `The vault's key is at generation ${current}, not ${generation}.`,
    );
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

/** Refuses keys for anything but the account's hosts, or two for one host. */
async function requireHosts(
  trx: Transaction<Tables>,
  userId: string,
  keys: WrappedKey[],
) {
  const wanted = new Set(keys.map((key) => key.hostKey));
  if (wanted.size !== keys.length)
    throw badRequest("Each host gets the key once.");
  const hosts = await trx
    .selectFrom("devices")
    .select("key")
    .where("user_id", "=", userId)
    .where("role", "=", "host")
    .where("key", "in", [...wanted])
    .execute();
  if (hosts.length !== wanted.size)
    throw badRequest(
      "A key is for a device that is not a host on the account.",
    );
}

function wrapRows(
  { userId, hostKey }: Asker,
  generation: number,
  keys: WrappedKey[],
) {
  return keys.map((key) => ({
    user_id: userId,
    host_key: key.hostKey,
    generation,
    wrapped: key.wrapped,
    wrapped_by: hostKey,
  }));
}

export async function createVault(
  db: Kysely<Tables>,
  asker: Asker,
  { recoveryKey, recoveryWrap, key }: VaultCreate,
): Promise<void> {
  const { userId, hostKey } = asker;
  await db.transaction().execute(async (trx) => {
    const made = await trx
      .insertInto("vaults")
      .values({
        user_id: userId,
        recovery_key: recoveryKey,
        recovery_wrap: recoveryWrap,
        created_by: hostKey,
      })
      .onConflict((oc) => oc.column("user_id").doNothing())
      .returning("user_id")
      .executeTakeFirst();
    if (!made) throw conflict("The account already has a vault.");
    await trx
      .insertInto("vault_keys")
      .values(wrapRows(asker, 1, [{ hostKey, wrapped: key }]))
      .execute();
    await appendEvent(trx, { userId, type: "vault.changed" });
    await audit(trx, {
      userId,
      actor: `device:${hostKey}`,
      action: "vault.created",
    });
  });
}

/** A holder seals the key for other hosts; a host restoring the vault from its recovery key seals it only for itself. */
export async function addKeys(
  db: Kysely<Tables>,
  asker: Asker,
  { generation, keys }: VaultKeysAdd,
): Promise<void> {
  const { userId, hostKey } = asker;
  await db.transaction().execute(async (trx) => {
    atCurrent((await lockedVault(trx, userId)).generation, generation);
    const forItself = keys.length === 1 && keys[0]?.hostKey === hostKey;
    if (!forItself) await requireHolder(trx, asker, generation);
    await requireHosts(trx, userId, keys);
    await trx
      .insertInto("vault_keys")
      .values(wrapRows(asker, generation, keys))
      .onConflict((oc) =>
        oc.columns(["user_id", "host_key"]).doUpdateSet((eb) => ({
          generation: eb.ref("excluded.generation"),
          wrapped: eb.ref("excluded.wrapped"),
          wrapped_by: eb.ref("excluded.wrapped_by"),
          created_at: sql<Date>`now()`,
        })),
      )
      .execute();
    await appendEvent(trx, { userId, type: "vault.changed" });
    for (const key of keys)
      await audit(trx, {
        userId,
        actor: `device:${hostKey}`,
        action: "vault.key_added",
        subject: key.hostKey,
        detail: { generation },
      });
  });
}

export async function rotateVault(
  db: Kysely<Tables>,
  asker: Asker,
  { from, recoveryWrap, keys, entries }: VaultRotate,
): Promise<void> {
  const { userId, hostKey } = asker;
  if (!keys.some((key) => key.hostKey === hostKey))
    throw badRequest("The new key is sealed for this host too.");
  await db.transaction().execute(async (trx) => {
    atCurrent((await lockedVault(trx, userId)).generation, from);
    await requireHolder(trx, asker, from);
    await requireHosts(trx, userId, keys);
    const current = await trx
      .selectFrom("vault_entries")
      .select(["id", "revision"])
      .where("user_id", "=", userId)
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
      .updateTable("vaults")
      .set({
        generation: next,
        rotate: false,
        recovery_wrap: recoveryWrap,
        rotated_at: sql<Date>`now()`,
      })
      .where("user_id", "=", userId)
      .execute();
    await trx.deleteFrom("vault_keys").where("user_id", "=", userId).execute();
    await trx
      .insertInto("vault_keys")
      .values(wrapRows(asker, next, keys))
      .execute();
    if (entries.length)
      await sql`
        update vault_entries
        set sealed = resealed.sealed, generation = ${next}, revision = revision + 1,
          updated_by = ${hostKey}, updated_at = now()
        from unnest(${entries.map((entry) => entry.id)}::text[], ${entries.map((entry) => entry.sealed)}::text[])
          as resealed (id, sealed)
        where vault_entries.id = resealed.id and vault_entries.user_id = ${userId}`.execute(
        trx,
      );
    await appendEvent(trx, { userId, type: "vault.changed" });
    await audit(trx, {
      userId,
      actor: `device:${hostKey}`,
      action: "vault.rotated",
      detail: { generation: next, hosts: keys.length, entries: entries.length },
    });
  });
}

export async function putEntry(
  db: Kysely<Tables>,
  asker: Asker,
  id: string,
  { plugin, generation, sealed, revision }: VaultEntryPut,
): Promise<VaultEntry> {
  const { userId, hostKey } = asker;
  return db.transaction().execute(async (trx) => {
    atCurrent((await lockedVault(trx, userId)).generation, generation);
    await requireHolder(trx, asker, generation);
    const existing = await trx
      .selectFrom("vault_entries")
      .select(["user_id", "revision"])
      .where("id", "=", id)
      .executeTakeFirst();
    if (existing && existing.user_id !== userId)
      throw conflict("That entry id is taken.");
    if ((existing?.revision ?? 0) !== revision)
      throw conflict(
        existing
          ? `The entry is at revision ${existing.revision}, not ${revision}.`
          : "The vault has no such entry; add it at revision 0.",
      );
    let row;
    if (existing) {
      row = await trx
        .updateTable("vault_entries")
        .set({
          plugin,
          generation,
          sealed,
          revision: revision + 1,
          updated_by: hostKey,
          updated_at: sql<Date>`now()`,
        })
        .where("id", "=", id)
        .returning(ENTRY_COLUMNS)
        .executeTakeFirstOrThrow();
    } else {
      const held = await trx
        .selectFrom("vault_entries")
        .select((eb) => eb.fn.countAll<string>().as("count"))
        .where("user_id", "=", userId)
        .executeTakeFirstOrThrow();
      if (Number(held.count) >= MAX_ENTRIES)
        throw conflict(`A vault holds at most ${MAX_ENTRIES} entries.`);
      row = await trx
        .insertInto("vault_entries")
        .values({
          id,
          user_id: userId,
          plugin,
          generation,
          sealed,
          revision: 1,
          updated_by: hostKey,
        })
        .returning(ENTRY_COLUMNS)
        .executeTakeFirstOrThrow();
    }
    await appendEvent(trx, { userId, type: "vault.changed" });
    await audit(trx, {
      userId,
      actor: `device:${hostKey}`,
      action: "vault.entry_written",
      subject: id,
      detail: { plugin, generation, revision: row.revision },
    });
    return toEntry(row);
  });
}

export async function removeEntry(
  db: Kysely<Tables>,
  { userId, hostKey }: Asker,
  id: string,
  revision: number,
): Promise<void> {
  await db.transaction().execute(async (trx) => {
    await lockedVault(trx, userId);
    const removed = await trx
      .deleteFrom("vault_entries")
      .where("id", "=", id)
      .where("user_id", "=", userId)
      .where("revision", "=", revision)
      .returning("plugin")
      .executeTakeFirst();
    if (!removed) {
      const existing = await trx
        .selectFrom("vault_entries")
        .select("revision")
        .where("id", "=", id)
        .where("user_id", "=", userId)
        .executeTakeFirst();
      if (!existing) throw notFound("No entry with that id is in the vault.");
      throw conflict(
        `The entry is at revision ${existing.revision}, not ${revision}.`,
      );
    }
    await appendEvent(trx, { userId, type: "vault.changed" });
    await audit(trx, {
      userId,
      actor: `device:${hostKey}`,
      action: "vault.entry_removed",
      subject: id,
      detail: { plugin: removed.plugin, revision },
    });
  });
}

export async function addUses(
  db: Kysely<Tables>,
  { userId, hostKey }: Asker,
  { uses }: VaultUsesAdd,
): Promise<void> {
  if (uses.some((use) => Date.parse(use.lastAt) < Date.parse(use.firstAt)))
    throw badRequest("A use's lastAt is not before its firstAt.");
  await db
    .insertInto("vault_uses")
    .values(
      uses.map((use) => ({
        user_id: userId,
        entry_id: use.entryId,
        plugin: use.plugin,
        host_key: hostKey,
        actor: use.actor,
        count: use.count,
        first_at: use.firstAt,
        last_at: use.lastAt,
      })),
    )
    .execute();
}
