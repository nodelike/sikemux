import type {
  OrgRole,
  OrgVault,
  OrgVaultEntry,
  OrgVaultState,
} from "@sikemux/protocol";
import { sql, type Kysely } from "kysely";

import { isStaff } from "../account/staff.ts";
import type { Tables } from "../db.ts";
import { notFound } from "../http.ts";
import type { Presence } from "../live/hub.ts";

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

export const NOT_IN_ORG =
  "Your account is in no org with that id, or plans are not open to it yet.";

export interface Member {
  /** The membership, as `org_users` keeps it. */
  id: string;
  role: OrgRole;
  active: boolean;
}

/** The caller's role in the org and whether its Team plan is paid, or 404 when they are not in it. */
export async function memberOf(
  db: Kysely<Tables>,
  live: boolean,
  orgId: string,
  userId: string,
): Promise<Member> {
  const row = await db
    .selectFrom("org_users")
    .select(["id", "role"])
    .select((eb) =>
      eb
        .exists(
          eb
            .selectFrom("subscriptions")
            .innerJoin(
              "billing_customers",
              "billing_customers.id",
              "subscriptions.customer_id",
            )
            .select(sql`1`.as("one"))
            .where("billing_customers.org_id", "=", orgId)
            .where("subscriptions.product", "=", "team")
            .where("subscriptions.status", "=", "active"),
        )
        .as("paid"),
    )
    .where("org_id", "=", orgId)
    .where("user_id", "=", userId)
    .executeTakeFirst();
  if (!row || (!live && !(await isStaff(db, userId))))
    throw notFound(NOT_IN_ORG);
  return { id: row.id, role: row.role, active: Boolean(row.paid) };
}

/** The keys of every host on the accounts of the org's members. */
export function memberHosts(db: Kysely<Tables>, orgId: string) {
  return db
    .selectFrom("devices")
    .innerJoin("org_users", "org_users.user_id", "devices.user_id")
    .select([
      "devices.key",
      "devices.name",
      "devices.user_id",
      "devices.last_seen_at",
      "org_users.role",
    ])
    .where("org_users.org_id", "=", orgId)
    .where("devices.role", "=", "host");
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
  after?: number,
): Promise<OrgVaultState> {
  const vault = await db
    .selectFrom("org_vaults")
    .select(["generation", "changes"])
    .where("org_id", "=", orgId)
    .executeTakeFirst();
  if (!vault) return { vault: null };
  const changes = Number(vault.changes);
  const since = after !== undefined && after <= changes ? after : null;
  const [keys, hosts, entries, ended] = await Promise.all([
    db
      .selectFrom("org_vault_keys")
      .select(["host_key", "generation", "wrapped"])
      .where("org_id", "=", orgId)
      .orderBy("created_at")
      .orderBy("host_key")
      .execute(),
    memberHosts(db, orgId)
      .orderBy("devices.created_at")
      .orderBy("devices.key")
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
  const members = new Set(hosts.map((host) => host.key));
  const current = keys.filter(
    (key) => key.generation === vault.generation && members.has(key.host_key),
  );
  const holders = new Set(current.map((key) => key.host_key));
  const state: OrgVault = {
    generation: vault.generation,
    key: current.find((key) => key.host_key === hostKey)?.wrapped ?? null,
    holders: [...holders],
    waiting: [...members].filter((key) => !holders.has(key)),
    rotate: keys.some((key) => !members.has(key.host_key)),
    holding:
      role === "user"
        ? []
        : hosts
            .filter((host) => holders.has(host.key))
            .map((host) => ({
              hostKey: host.key,
              computer: host.name,
              role: host.role,
              yours: host.user_id === userId,
              online: presence.online(host.key),
              lastSeenAt: host.last_seen_at?.toISOString() ?? null,
            })),
    entries: entries.map(toEntry),
    ended,
    changes,
    since,
  };
  return { vault: state };
}
