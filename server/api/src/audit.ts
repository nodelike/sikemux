import type { Kysely } from "kysely";

import type { Tables } from "./db.ts";

/** Who did it: a person, a device acting for itself, Clerk's webhook, the API on its own, or a deploy. */
export type Actor =
  `user:${string}` | `device:${string}` | "clerk" | "system" | "deploy";

export type AuditAction =
  | "device.registered"
  | "device.updated"
  | "device.removed"
  | "push.enabled"
  | "push.disabled"
  | "join.issued"
  | "host_sign_in.approved"
  | "host_sign_in.refused"
  | "account.deletion_requested"
  | "account.deleted_in_clerk"
  | "account.restored"
  | "account.apple_revoked"
  | "account.apple_revoke_failed"
  | "account.purged"
  | "webhook.user_deleted"
  | "update.published"
  | "update.promoted"
  | "update.withdrawn"
  | "update.rolled_back";

export interface AuditEntry {
  /** The account it happened to, or null for what belongs to none. */
  userId: string | null;
  actor: Actor;
  action: AuditAction;
  subject?: string;
  detail?: Record<string, unknown>;
}

/** Adds a row to the audit log, in the caller's transaction when it passes one. */
export async function audit(
  db: Kysely<Tables>,
  { userId, actor, action, subject, detail }: AuditEntry,
): Promise<void> {
  await db
    .insertInto("audit")
    .values({
      user_id: userId,
      actor,
      action,
      subject: subject ?? null,
      ...(detail ? { detail: JSON.stringify(detail) } : {}),
    })
    .execute();
}
