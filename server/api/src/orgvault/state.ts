import type {
  OrgRole,
  OrgVault,
  OrgVaultEntry,
  OrgVaultState,
} from "@sikemux/protocol";
import type { Kysely } from "kysely";

import {
  holdsVaults,
  mayHoldVaults,
  mayUseTeam,
  teamActive,
  VAULT_SOON,
} from "../billing/team.ts";
import type { Tables } from "../db.ts";
import { forbidden, notFound } from "../http.ts";
import type { Presence } from "../live/hub.ts";

/** Names members by their user ids, leaving out whoever cannot be named. */
export type Namer = (
  userIds: string[],
) => Promise<Map<string, { name: string | null }>>;

/** A shared project's store of rules and knowledge, which agents read. */
export const PROJECT_MEMORY = "project-memory";
/** One member's rules for a shared project, waiting for an owner or admin to keep or drop them. */
export const PROJECT_MEMORY_WAITING = "project-memory-waiting";
/** The waiting rules of a shared project that an owner or admin dropped. */
export const PROJECT_MEMORY_DROPS = "project-memory-drops";
export const PROJECT_KINDS = [
  PROJECT_MEMORY,
  PROJECT_MEMORY_WAITING,
  PROJECT_MEMORY_DROPS,
];

type EntryRow = Pick<
  Tables["org_vault_entries"],
  "id" | "kind" | "generation" | "sealed" | "revision" | "updated_by"
> & { updated_at: Date };

export const ENTRY_COLUMNS = [
  "id",
  "kind",
  "generation",
  "sealed",
  "revision",
  "updated_by",
  "updated_at",
] as const;

export function toEntry(row: EntryRow): OrgVaultEntry {
  return {
    id: row.id,
    kind: row.kind,
    generation: row.generation,
    sealed: row.sealed,
    revision: row.revision,
    updatedBy: row.updated_by,
    updatedAt: row.updated_at.toISOString(),
  };
}

export const NOT_IN_ORG = "Your account is in no org with that id.";

export interface Member {
  /** The membership, as `org_users` keeps it. */
  id: string;
  role: OrgRole;
  active: boolean;
}

/**
 * The caller's role in the org and whether it and they have Team now, or 404 when they are not in
 * it. Without Team a member still reads what the vault holds, as does one whose host holds its key
 * though vaults are not open to them.
 */
export async function memberOf(
  db: Kysely<Tables>,
  orgId: string,
  userId: string,
): Promise<Member> {
  const row = await db
    .selectFrom("orgs")
    .innerJoin("org_users", "org_users.org_id", "orgs.id")
    .select(["org_users.id", "org_users.role"])
    .select((eb) => teamActive(eb).as("active"))
    .where("orgs.id", "=", orgId)
    .where("org_users.user_id", "=", userId)
    .executeTakeFirst();
  if (!row) throw notFound(NOT_IN_ORG);
  const open = await holdsVaults(db, userId);
  if (!open && !(await holdsKey(db, orgId, userId)))
    throw forbidden(VAULT_SOON);
  const active = open && Boolean(row.active) && (await mayUseTeam(db, userId));
  return { id: row.id, role: row.role, active };
}

/** Whether a host on the account holds the org's key. */
async function holdsKey(db: Kysely<Tables>, orgId: string, userId: string) {
  return Boolean(
    await db
      .selectFrom("org_vault_keys")
      .innerJoin("devices", "devices.key", "org_vault_keys.host_key")
      .select("devices.key")
      .where("org_vault_keys.org_id", "=", orgId)
      .where("devices.user_id", "=", userId)
      .executeTakeFirst(),
  );
}

/**
 * The members' hosts that may hold the org's key, hold it, or were let in, so closing vaults ends
 * no one's.
 */
export function memberHosts(db: Kysely<Tables>, orgId: string) {
  return db
    .selectFrom("devices")
    .innerJoin("org_users", "org_users.user_id", "devices.user_id")
    .innerJoin("users", "users.id", "devices.user_id")
    .select([
      "devices.key",
      "devices.name",
      "devices.user_id",
      "org_users.id as member",
      "devices.last_seen_at",
      "org_users.role",
    ])
    .where("org_users.org_id", "=", orgId)
    .where("devices.role", "=", "host")
    .where((eb) =>
      eb.or([
        mayHoldVaults(eb),
        eb.exists(
          eb
            .selectFrom("org_vault_keys")
            .select("host_key")
            .where("org_id", "=", orgId)
            .whereRef("host_key", "=", "devices.key"),
        ),
        eb.exists(
          eb
            .selectFrom("org_vault_vouches")
            .select("host_key")
            .where("org_id", "=", orgId)
            .whereRef("host_key", "=", "devices.key")
            .whereRef("member", "=", "org_users.id"),
        ),
      ]),
    );
}

/** Whether `hostKey`'s wrap among `wraps` may open: sealed by itself, or by another holder. */
function mayOpenOwnWrap(
  hostKey: string,
  wraps: { host_key: string; wrapped_by: string }[],
): boolean {
  const own = wraps.find((wrap) => wrap.host_key === hostKey);
  return (
    own !== undefined &&
    (own.wrapped_by === hostKey ||
      wraps.some((wrap) => wrap.host_key === own.wrapped_by))
  );
}

export interface Viewer {
  hostKey: string;
  userId: string;
  role: OrgRole;
}

/**
 * The org's vault as the viewer's host sees it: the key sealed for it, and which members' hosts
 * hold it or wait. With `after`, only the entries changed or removed since that many changes.
 */
export async function orgVaultState(
  db: Kysely<Tables>,
  orgId: string,
  { hostKey, userId, role }: Viewer,
  presence: Presence,
  people: Namer,
  after?: number,
): Promise<OrgVaultState> {
  const vault = await db
    .selectFrom("org_vaults")
    .select(["generation", "changes", "remade_at"])
    .where("org_id", "=", orgId)
    .executeTakeFirst();
  if (!vault) return { vault: null };
  const changes = Number(vault.changes);
  const since = after !== undefined && after <= changes ? after : null;
  const [keys, hosts, vouches, entries, ended] = await Promise.all([
    db
      .selectFrom("org_vault_keys")
      .select(["host_key", "generation", "wrapped", "wrapped_by"])
      .where("org_id", "=", orgId)
      .orderBy("created_at")
      .orderBy("host_key")
      .execute(),
    memberHosts(db, orgId)
      .orderBy("devices.created_at")
      .orderBy("devices.key")
      .execute(),
    db
      .selectFrom("org_vault_vouches")
      .select(["host_key", "member", "by_key", "by_member", "at", "signature"])
      .select((eb) =>
        eb
          .case()
          .when("host_key", "=", hostKey)
          .then(eb.ref("introduction"))
          .else(null)
          .end()
          .as("introduction"),
      )
      .where("org_id", "=", orgId)
      .orderBy("host_key")
      .orderBy("by_key")
      .execute(),
    db
      .selectFrom("org_vault_entries")
      .select(ENTRY_COLUMNS)
      .where("org_id", "=", orgId)
      .where("change", ">", String(since ?? 0))
      .orderBy("created_at")
      .orderBy("id")
      .execute(),
    db
      .selectFrom("org_vault_ended")
      .select(["id", "revision"])
      .where("org_id", "=", orgId)
      .where("change", ">", String(since ?? 0))
      .orderBy("id")
      .execute(),
  ]);
  const members = new Map(hosts.map((host) => [host.key, host.member]));
  const current = keys.filter(
    (key) => key.generation === vault.generation && members.has(key.host_key),
  );
  const holders = new Set(current.map((key) => key.host_key));
  const own = current.find((key) => key.host_key === hostKey);
  const key = own?.wrapped ?? null;
  // A host that opened the key reads only what changed. A full read is from one that may not have,
  // as a host takes the key only from hosts it trusts, which nothing here says.
  const opens =
    since === null
      ? own?.wrapped_by === hostKey
      : mayOpenOwnWrap(hostKey, current);
  const holding =
    role === "user" || opens
      ? null
      : hosts.filter(
          (host) =>
            host.key !== hostKey &&
            holders.has(host.key) &&
            (host.user_id === userId || host.role !== "user"),
        );
  const named =
    holding && holding.length > 0
      ? await people([...new Set(holding.map((host) => host.user_id))])
      : new Map<string, { name: string | null }>();
  const state: OrgVault = {
    generation: vault.generation,
    key,
    holders: [...holders],
    waiting: [...members.keys()].filter((key) => !holders.has(key)),
    rotate: keys.some((key) => !members.has(key.host_key)),
    ...(holding && {
      holding: holding.map((host) => ({
        hostKey: host.key,
        computer: host.name,
        member: named.get(host.user_id)?.name ?? null,
        role: host.role,
        yours: host.user_id === userId,
        online: presence.online(host.key),
        lastSeenAt: host.last_seen_at?.toISOString() ?? null,
      })),
    }),
    remadeAt: vault.remade_at?.toISOString() ?? null,
    entries: entries.map(toEntry),
    ended,
    changes,
    since,
    hosts: hosts.map((host) => ({
      hostKey: host.key,
      member: host.member,
      ...((role !== "user" || host.user_id === userId) && {
        computer: host.name,
      }),
    })),
    vouches: vouches
      .filter((vouch) => members.get(vouch.host_key) === vouch.member)
      .map((vouch) => ({
        hostKey: vouch.host_key,
        member: vouch.member,
        by: vouch.by_key,
        byMember: vouch.by_member,
        at: Number(vouch.at),
        signature: vouch.signature,
        ...(vouch.introduction !== null && {
          introduction: vouch.introduction,
        }),
      })),
  };
  return { vault: state };
}
