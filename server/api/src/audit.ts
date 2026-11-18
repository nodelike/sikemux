import type { Kysely } from "kysely";

import type { Tables } from "./db.ts";

/** Who did it: a person, a device acting for itself, Clerk's webhook, the API on its own, or a deploy. */
export type Actor =
  `user:${string}` | `device:${string}` | "clerk" | "system" | "deploy";

export type AuditAction =
  | "device.registered"
  | "device.updated"
  | "device.renamed"
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
  | "account.folded"
  | "webhook.user_deleted"
  | "update.published"
  | "update.promoted"
  | "update.withdrawn"
  | "update.rolled_back"
  | "vault.created"
  | "vault.rotated"
  | "vault.key_added"
  | "vault.entry_written"
  | "vault.entry_removed"
  | "org_vault.created"
  | "org_vault.key_added"
  | "org_vault.rotated"
  | "org_vault.entry_written"
  | "org_vault.entry_removed"
  | "mods.purged"
  | "billing.checkout_opened"
  | "billing.subscription_changed"
  | "billing.change_taken_back"
  | "billing.subscription_canceled"
  | "billing.subscription_resumed"
  | "billing.refund_requested"
  | "org.created"
  | "org.deleted"
  | "org.joined"
  | "org.left"
  | "org.member_removed"
  | "org.role_changed"
  | "org.invite_created"
  | "org.invite_revoked"
  | "org.team_created"
  | "org.team_renamed"
  | "org.team_deleted"
  | "org.team_member_added"
  | "org.team_member_removed"
  | "org.domain_added"
  | "org.domain_verified"
  | "org.domain_joining_changed"
  | "org.domain_removed"
  | "org.policy_changed"
  | "share.added"
  | "share.moved"
  | "share.removed";

interface AuditEntry {
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
