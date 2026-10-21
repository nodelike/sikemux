import type { DeviceRole } from "@sikemux/protocol";
import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";
import { notFound } from "../http.ts";

const MISSING: Record<DeviceRole | "any", string> = {
  host: "None of your hosts has that key.",
  client: "None of your phones has that key.",
  any: "None of your devices has that key.",
};

/**
 * The signed-in account's device with this key. Anything else, including the account's device
 * of another role, answers 404 the same way, so a caller learns nothing about keys it does not own.
 */
export async function ownedDevice(
  db: Kysely<Tables>,
  userId: string,
  key: string,
  { role, lock = false }: { role?: DeviceRole; lock?: boolean } = {},
) {
  let query = db
    .selectFrom("devices")
    .selectAll()
    .where("key", "=", key)
    .where("user_id", "=", userId);
  if (role) query = query.where("role", "=", role);
  const device = await (lock ? query.forUpdate() : query).executeTakeFirst();
  if (!device) throw notFound(MISSING[role ?? "any"]);
  return device;
}
