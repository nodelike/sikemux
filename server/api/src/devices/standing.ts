import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";

/** Every device on an account that stands: one that is not on its way to being deleted. */
export function standingDevices(db: Kysely<Tables>) {
  return db
    .selectFrom("devices")
    .innerJoin("users", "users.id", "devices.user_id")
    .where("users.deleted_at", "is", null);
}

/** The hosts among `keys` still on an account that stands, with the account each is on. */
export async function hostAccounts(
  db: Kysely<Tables>,
  keys: string[],
): Promise<Map<string, string>> {
  if (!keys.length) return new Map();
  const rows = await standingDevices(db)
    .select(["devices.key", "devices.user_id"])
    .where("devices.key", "in", keys)
    .where("devices.role", "=", "host")
    .execute();
  return new Map(rows.map((row) => [row.key, row.user_id]));
}
