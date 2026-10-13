import type { DeviceRole, RevokeReason } from "@sikemux/protocol";
import { sql, type Transaction } from "kysely";

import { audit, type Actor } from "../audit.ts";
import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";

export interface Removal {
  userId: string;
  key: string;
  reason: RevokeReason;
  actor: Actor;
  detail?: Record<string, unknown>;
}

export interface Removed {
  key: string;
  role: DeviceRole;
  clerkSessionId: string | null;
}

/**
 * Takes a device off its account: the row moves to a tombstone that keeps its cursor, so a
 * device that was offline still learns why it is gone, and the account hears device.revoked.
 */
export async function removeDevice(
  trx: Transaction<Tables>,
  { userId, key, reason, actor, detail = {} }: Removal,
): Promise<Removed | undefined> {
  const row = await trx
    .deleteFrom("devices")
    .where("key", "=", key)
    .where("user_id", "=", userId)
    .returning(["key", "role", "name", "acked_event_id", "clerk_session_id"])
    .executeTakeFirst();
  if (!row) return undefined;
  await trx
    .insertInto("removed_devices")
    .values({
      key,
      user_id: userId,
      role: row.role,
      reason,
      acked_event_id: row.acked_event_id,
      clerk_session_id: row.clerk_session_id,
      clerk_retry_at:
        reason === "removed" && row.clerk_session_id ? sql<Date>`now()` : null,
    })
    .execute();
  await appendEvent(trx, {
    userId,
    type: "device.revoked",
    subject: key,
    subjectRole: row.role as DeviceRole,
    reason,
  });
  await audit(trx, {
    userId,
    actor,
    action: "device.removed",
    subject: key,
    detail: {
      role: row.role,
      name: row.name,
      reason,
      ...detail,
    },
  });
  return {
    key,
    role: row.role as DeviceRole,
    clerkSessionId: row.clerk_session_id,
  };
}
