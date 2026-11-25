import type {
  OrgVaultCreate,
  OrgVaultEntry,
  OrgVaultEntryPut,
  OrgVaultEntryRead,
  OrgVaultKeysAdd,
  OrgVaultRemake,
  OrgVaultRotate,
  OrgWrappedKey,
} from "@sikemux/protocol";
import { sql, type Transaction } from "kysely";

import { audit, type AuditAction } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvents } from "../events/log.ts";
import type { Presence } from "../live/hub.ts";
import { ApiFailure, badRequest, forbidden, notFound } from "../http.ts";
import {
  ENTRY_COLUMNS,
  memberHosts,
  PROJECT_KINDS,
  PROJECT_MEMORY,
  PROJECT_MEMORY_DROPS,
  PROJECT_MEMORY_WAITING,
  toEntry,
  type Member,
} from "./state.ts";
import { letIn, vouchedKeys } from "./vouches.ts";

/**
 * The most entries of each project kind an org's vault holds, so they never crowd out the rest.
 * With every other kind's they add up to the 500 one rotation reseals.
 */
const MAX_ENTRIES: Record<string, { most: number; full: string }> = {
  [PROJECT_MEMORY]: {
    most: 100,
    full: "An org keeps rules for at most 100 projects.",
  },
  [PROJECT_MEMORY_WAITING]: {
    most: 200,
    full: "An org keeps rules waiting for an admin in at most 200 projects.",
  },
  [PROJECT_MEMORY_DROPS]: {
    most: 100,
    full: "An org keeps dropped rules for at most 100 projects.",
  },
};
/** The most entries of every other kind together, such as model keys. */
const MAX_OTHER_ENTRIES = {
  most: 100,
  full: "An org keeps at most 100 model keys.",
};

export interface Asker {
  orgId: string;
  userId: string;
  hostKey: string;
}

/** Refuses a host whose Sikemux may seal the key for hosts nobody let in. */
function requireApprovedOnly({ approvedOnly }: { approvedOnly?: boolean }) {
  if (!approvedOnly)
    throw forbidden(
      "This computer's Sikemux is too old for the org's vault. Update Sikemux here first.",
    );
}

function conflict(message: string): ApiFailure {
  return new ApiFailure(409, "conflict", message);
}

/** The org's vault, locked until the transaction ends so its changes happen one at a time. */
export async function lockedVault(trx: Transaction<Tables>, orgId: string) {
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

export async function requireHolder(
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

/**
 * Marks each removed entry one revision above its last, so a write from a host that read it
 * before it went is refused and only a host that saw it go can add an entry under its id.
 */
export async function keepEnded(
  trx: Transaction<Tables>,
  orgId: string,
  removed: { id: string; revision: number }[],
) {
  if (!removed.length) return;
  await trx
    .insertInto("org_vault_ended")
    .values(
      removed.map(({ id, revision }) => ({
        org_id: orgId,
        id,
        revision: revision + 1,
      })),
    )
    .onConflict((oc) =>
      oc.columns(["org_id", "id"]).doUpdateSet({
        revision: (eb) => eb.ref("excluded.revision"),
      }),
    )
    .execute();
}

export function auditOrg(
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
  body: OrgVaultCreate,
): Promise<void> {
  requireApprovedOnly(body);
  const made = await trx
    .insertInto("org_vaults")
    .values({ org_id: asker.orgId, created_by: asker.hostKey })
    .onConflict((oc) => oc.column("org_id").doNothing())
    .returning("org_id")
    .executeTakeFirst();
  if (!made) throw conflict("The org already has a vault.");
  await trx
    .insertInto("org_vault_keys")
    .values(wrapRows(asker, 1, [{ hostKey: asker.hostKey, wrapped: body.key }]))
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
 * any, anyone else's only for its own account's. A host that no host vouched for is left out, and
 * one that has a wrap keeps it.
 */
export async function addOrgKeys(
  trx: Transaction<Tables>,
  asker: Asker,
  member: Member,
  body: OrgVaultKeysAdd,
): Promise<void> {
  requireApprovedOnly(body);
  const { generation, keys, vouches } = body;
  atCurrent((await lockedVault(trx, asker.orgId)).generation, generation);
  await requireHolder(trx, asker, generation);
  await requireMemberHosts(trx, asker.orgId, keys);
  if (member.role === "user") await requireOwnHosts(trx, asker, keys);
  await letIn(trx, asker, member, vouches ?? []);
  const vouched = await vouchedKeys(trx, asker.orgId, keys);
  const added = vouched.length
    ? await trx
        .insertInto("org_vault_keys")
        .values(wrapRows(asker, generation, vouched))
        .onConflict((oc) => oc.columns(["org_id", "host_key"]).doNothing())
        .returning("host_key")
        .execute()
    : [];
  if (added.length || vouches?.length) await vaultChanged(trx, asker.orgId);
  if (!added.length) return;
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

/**
 * Every wrap is replaced by the new key's, so a host that left keeps only a key nothing is sealed
 * under. A host that neither held the key nor was vouched for is left out. Project entries that do
 * not open on the asking host are dropped, so no member's write can keep the key from being
 * replaced.
 */
export async function rotateOrgVault(
  trx: Transaction<Tables>,
  asker: Asker,
  member: Member,
  body: OrgVaultRotate,
): Promise<void> {
  requireApprovedOnly(body);
  const { from, keys, entries, drops = [], vouches } = body;
  const { orgId, hostKey } = asker;
  if (!keys.some((key) => key.hostKey === hostKey))
    throw badRequest("The new key is sealed for this host too.");
  atCurrent((await lockedVault(trx, orgId)).generation, from);
  await requireHolder(trx, asker, from);
  await requireMemberHosts(trx, orgId, keys);
  await letIn(trx, asker, member, vouches ?? []);
  const holders = await trx
    .selectFrom("org_vault_keys")
    .select("host_key")
    .where("org_id", "=", orgId)
    .where("generation", "=", from)
    .execute();
  const held = new Set(holders.map((holder) => holder.host_key));
  const vouched = new Set(await vouchedKeys(trx, orgId, keys));
  const kept = keys.filter((key) => held.has(key.hostKey) || vouched.has(key));
  const current = await trx
    .selectFrom("org_vault_entries")
    .select(["id", "kind", "revision"])
    .where("org_id", "=", orgId)
    .execute();
  const listed = new Map(current.map((row) => [row.id, row]));
  const given = [...entries, ...drops];
  const named = new Set(given.map((entry) => entry.id));
  if (
    named.size !== given.length ||
    given.length !== current.length ||
    given.some((entry) => listed.get(entry.id)?.revision !== entry.revision)
  )
    throw conflict(
      "The entries are not every entry in the vault at its current revision.",
    );
  const dropped = new Set(drops.map((entry) => entry.id));
  if (
    current.some(
      (row) => dropped.has(row.id) && !PROJECT_KINDS.includes(row.kind),
    )
  )
    throw forbidden(
      "Replacing the key removes only a shared project's entries.",
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
    .values(wrapRows(asker, next, kept))
    .execute();
  if (drops.length)
    await trx
      .deleteFrom("org_vault_entries")
      .where("org_id", "=", orgId)
      .where("id", "in", [...dropped])
      .execute();
  await keepEnded(trx, orgId, drops);
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
    hosts: kept.length,
    entries: entries.length,
    dropped: drops.length,
  });
}

/**
 * For when no computer can open the vault any more: everything sealed under the old key is
 * dropped, with what any host vouched for, and the asking host's key at the next generation is the
 * only one. Refused when the asking host sealed the key for itself, or while a computer that could
 * let it in may be online.
 */
export async function remakeOrgVault(
  trx: Transaction<Tables>,
  asker: Asker,
  body: OrgVaultRemake,
  presence: Presence,
): Promise<void> {
  requireApprovedOnly(body);
  const { from, key } = body;
  const { orgId, userId, hostKey } = asker;
  atCurrent((await lockedVault(trx, orgId)).generation, from);
  const holders = await memberHosts(trx, orgId)
    .innerJoin("org_vault_keys", (join) =>
      join
        .onRef("org_vault_keys.host_key", "=", "devices.key")
        .on("org_vault_keys.org_id", "=", orgId)
        .on("org_vault_keys.generation", "=", from),
    )
    .select("org_vault_keys.wrapped_by")
    .orderBy("devices.created_at")
    .execute();
  if (
    holders.some((host) => host.key === hostKey && host.wrapped_by === hostKey)
  )
    throw conflict(
      "The vault already opens on this host, so there is nothing to make again.",
    );
  const awake = holders.find(
    (host) =>
      host.key !== hostKey &&
      (host.user_id === userId || host.role !== "user") &&
      presence.mayBeOnline(host.key, host.last_seen_at),
  );
  if (awake)
    throw conflict(
      `${awake.name} may be online and has the vault. Paste this host's code there and nothing is lost.`,
    );
  const dropped = await trx
    .deleteFrom("org_vault_entries")
    .where("org_id", "=", orgId)
    .returning(["id", "revision"])
    .execute();
  await keepEnded(trx, orgId, dropped);
  const wraps = await trx
    .deleteFrom("org_vault_keys")
    .where("org_id", "=", orgId)
    .returning("host_key")
    .execute();
  await trx
    .deleteFrom("org_vault_vouches")
    .where("org_id", "=", orgId)
    .execute();
  const next = from + 1;
  await trx
    .updateTable("org_vaults")
    .set({
      generation: next,
      rotated_at: sql<Date>`now()`,
      remade_at: sql<Date>`now()`,
    })
    .where("org_id", "=", orgId)
    .execute();
  await trx
    .insertInto("org_vault_keys")
    .values(wrapRows(asker, next, [{ hostKey, wrapped: key }]))
    .execute();
  await vaultChanged(trx, orgId);
  await auditOrg(trx, asker, "org_vault.remade", {
    generation: next,
    entries: dropped.length,
    wraps: wraps.length,
  });
}

/**
 * Refuses a write while a host that left still has a wrap, until an owner's or admin's host
 * replaces the key, so nothing new is sealed under a key it holds and no write races that.
 */
async function requireNoneLeft(trx: Transaction<Tables>, orgId: string) {
  const left = await trx
    .selectFrom("org_vault_keys")
    .select("host_key")
    .where("org_id", "=", orgId)
    .where(
      "host_key",
      "not in",
      memberHosts(trx, orgId).clearSelect().select("devices.key"),
    )
    .executeTakeFirst();
  if (left)
    throw new ApiFailure(
      503,
      "unavailable",
      "A computer that held the org's key left, so it is being replaced. An owner's or admin's computer does that once online.",
    );
}

async function unmoved(
  trx: Transaction<Tables>,
  orgId: string,
  read: OrgVaultEntryRead[],
): Promise<void> {
  if (read.length === 0) return;
  const ids = read.map((entry) => entry.id);
  const [held, ended] = await Promise.all([
    trx
      .selectFrom("org_vault_entries")
      .select(["id", "revision"])
      .where("org_id", "=", orgId)
      .where("id", "in", ids)
      .execute(),
    trx
      .selectFrom("org_vault_ended")
      .select(["id", "revision"])
      .where("org_id", "=", orgId)
      .where("id", "in", ids)
      .execute(),
  ]);
  const now = new Map(held.map((entry) => [entry.id, entry.revision]));
  const gone = new Set(ended.map((entry) => entry.id));
  const moved = read.find(
    (entry) =>
      (now.get(entry.id) ?? 0) !== entry.revision ||
      (!now.has(entry.id) && gone.has(entry.id)),
  );
  if (moved)
    throw conflict(
      `Entry ${moved.id} moved on from revision ${moved.revision} after it was read.`,
    );
}

export async function putOrgEntry(
  trx: Transaction<Tables>,
  asker: Asker,
  id: string,
  { kind, generation, sealed, revision, checked = [] }: OrgVaultEntryPut,
): Promise<OrgVaultEntry> {
  const { orgId, hostKey } = asker;
  atCurrent((await lockedVault(trx, orgId)).generation, generation);
  await requireHolder(trx, asker, generation);
  await requireNoneLeft(trx, orgId);
  await unmoved(trx, orgId, checked);
  const existing = await trx
    .selectFrom("org_vault_entries")
    .select(["revision", "kind"])
    .where("org_id", "=", orgId)
    .where("id", "=", id)
    .executeTakeFirst();
  if (existing && existing.kind !== kind)
    throw forbidden("An entry keeps the kind it was first written with.");
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
    const { most, full } = cap ?? MAX_OTHER_ENTRIES;
    if (Number(held.count) >= most) throw forbidden(full);
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
