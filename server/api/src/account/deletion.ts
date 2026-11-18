import release from "@sikemux/protocol/release.json" with { type: "json" };
import { sql, type Kysely, type Transaction } from "kysely";

import { audit, type Actor } from "../audit.ts";
import type { Tables } from "../db.ts";
import { removeDevice } from "../devices/removal.ts";
import { holdsOpenSubscriptions } from "../billing/deleted.ts";
import { payerLeft } from "../billing/payer.ts";
import { appendEvent } from "../events/log.ts";
import { conflict } from "../http.ts";
import type { Logger } from "../log.ts";
import { erasedFromVault } from "../orgvault/change.ts";
import { dropUnshared } from "../orgvault/projects.ts";
import { retryLater } from "../retry.ts";
import type { ClerkBackend } from "./clerk.ts";

/**
 * How long a deleted account can still be restored. Then the sign-in is deleted in Clerk and
 * the account's records erased; until then its tombstones tell offline hosts it is gone.
 */
export const PURGE_AFTER_DAYS = release.account.restorableDays;

interface DeletionRequest {
  userId: string;
  actor: Actor;
  via: string;
  /** Whether the user is already gone from Clerk, as when Clerk's webhook reports it. */
  deletedInClerk: boolean;
}

interface Deletion {
  requestedAt: Date;
  deletesAt: Date;
  deletedInClerk: boolean;
}

/** After a call to Clerk failed: what to record on the row so the sweeper tries again later. */
function clerkFailed(
  log: Logger,
  what: string,
  failure: Record<string, unknown> & { error: unknown; attempts: number },
) {
  return {
    clerk_attempts: failure.attempts,
    clerk_retry_at: retryLater(log, { service: "Clerk", what }, failure),
  };
}

/**
 * Deletes the account here, in one transaction: every device moves to a tombstone, the account
 * hears device.revoked for each and one account.deleted, and its tokens are refused from now on.
 * It leaves its orgs, and what it pays for is left for `endDeletedBilling` to end with the provider.
 * The sign-in stays in Clerk until the account can no longer be restored. A repeat finds the
 * account marked and changes nothing, except recording Clerk's deletion.
 */
export async function markDeleted(
  db: Kysely<Tables>,
  { userId, actor, via, deletedInClerk }: DeletionRequest,
): Promise<Deletion> {
  return db.transaction().execute(async (trx) => {
    await trx
      .insertInto("users")
      .values({ id: userId })
      .onConflict((oc) => oc.column("id").doNothing())
      .execute();
    const user = await trx
      .selectFrom("users")
      .select(["deleted_at", "purge_after", "clerk_deleted_at"])
      .where("id", "=", userId)
      .forNoKeyUpdate()
      .executeTakeFirstOrThrow();

    if (user.deleted_at) {
      if (deletedInClerk && !user.clerk_deleted_at)
        await recordClerkDeletion(trx, userId, actor);
      return {
        requestedAt: user.deleted_at,
        deletesAt: user.purge_after ?? user.deleted_at,
        deletedInClerk: deletedInClerk || user.clerk_deleted_at !== null,
      };
    }

    const orgs = await leaveOrgs(trx, userId, actor, {
      handOver: deletedInClerk,
    });
    const marked = await trx
      .updateTable("users")
      .set({
        billing_attempts: 0,
        billing_retry_at: (await holdsOpenSubscriptions(trx, userId))
          ? sql<Date>`now()`
          : null,
        deleted_at: sql<Date>`now()`,
        purge_after: sql<Date>`now() + make_interval(days => ${PURGE_AFTER_DAYS})`,
        clerk_deleted_at: deletedInClerk ? sql<Date>`now()` : null,
        clerk_attempts: 0,
        clerk_retry_at: deletedInClerk
          ? null
          : sql<Date>`now() + make_interval(days => ${PURGE_AFTER_DAYS})`,
      })
      .where("id", "=", userId)
      .returning(["deleted_at", "purge_after"])
      .executeTakeFirstOrThrow();
    const devices = await trx
      .selectFrom("devices")
      .select("key")
      .where("user_id", "=", userId)
      .orderBy("created_at")
      .execute();
    for (const { key } of devices)
      await removeDevice(trx, {
        userId,
        key,
        reason: "account_deleted",
        actor,
      });
    await appendEvent(trx, { userId, type: "account.deleted" });
    await audit(trx, {
      userId,
      actor,
      action: "account.deletion_requested",
      detail: { via, devices: devices.length, ...orgs },
    });
    if (deletedInClerk) await auditClerkDeletion(trx, userId, actor);
    return {
      requestedAt: marked.deleted_at ?? new Date(),
      deletesAt: marked.purge_after ?? new Date(),
      deletedInClerk,
    };
  });
}

/**
 * Takes the person out of every org they share with others. An org's only owner is refused,
 * unless `handOver`, when the org passes to its longest-standing admin, else member. An org they
 * are alone in stays theirs, with its pending invites revoked, until it is erased with them.
 */
async function leaveOrgs(
  trx: Transaction<Tables>,
  userId: string,
  actor: Actor,
  { handOver }: { handOver: boolean },
) {
  const why = "account_deleted";
  const memberships = await trx
    .selectFrom("org_users")
    .select("org_id")
    .where("user_id", "=", userId)
    .orderBy("org_id")
    .execute();
  const counts = { orgsLeft: 0, orgsHandedOver: 0, orgsKept: 0 };
  for (const { org_id: orgId } of memberships) {
    await trx
      .selectFrom("orgs")
      .select("id")
      .where("id", "=", orgId)
      .forUpdate()
      .execute();
    const members = await trx
      .selectFrom("org_users")
      .select(["user_id", "role"])
      .where("org_id", "=", orgId)
      .orderBy("joined_at")
      .orderBy("user_id")
      .execute();
    const others = members.filter((member) => member.user_id !== userId);
    if (!others.length) {
      await trx
        .updateTable("org_invites")
        .set({ revoked_at: sql<Date>`now()` })
        .where("org_id", "=", orgId)
        .where("accepted_at", "is", null)
        .where("revoked_at", "is", null)
        .execute();
      counts.orgsKept += 1;
      continue;
    }
    const mine = members.find((member) => member.user_id === userId);
    if (mine?.role === "owner" && !others.some((o) => o.role === "owner")) {
      if (!handOver)
        throw conflict(
          "You are the only owner of an org with other members: make one of them an owner first.",
        );
      const heir = others.find((o) => o.role === "admin") ?? others[0];
      if (heir) {
        await trx
          .updateTable("org_users")
          .set({ role: "owner" })
          .where("org_id", "=", orgId)
          .where("user_id", "=", heir.user_id)
          .execute();
        await audit(trx, {
          userId: heir.user_id,
          actor,
          action: "org.role_changed",
          subject: orgId,
          detail: { from: heir.role, to: "owner", why },
        });
      }
      counts.orgsHandedOver += 1;
    }
    const left = await trx
      .deleteFrom("org_users")
      .where("org_id", "=", orgId)
      .where("user_id", "=", userId)
      .returning("id")
      .executeTakeFirst();
    if (left) await erasedFromVault(trx, orgId, left.id);
    await payerLeft(trx, orgId, userId);
    await dropUnshared(trx, orgId);
    await audit(trx, {
      userId,
      actor,
      action: "org.left",
      subject: orgId,
      detail: { role: mine?.role, why },
    });
    counts.orgsLeft += 1;
  }
  return counts;
}

/**
 * Takes back a deletion while the sign-in is still in Clerk. Devices removed by the deletion stay
 * removed: each signs in again. Returns false when Clerk has already deleted the sign-in.
 */
export async function restoreAccount(
  db: Kysely<Tables>,
  userId: string,
): Promise<boolean> {
  return db.transaction().execute(async (trx) => {
    const user = await trx
      .selectFrom("users")
      .select(["deleted_at", "clerk_deleted_at"])
      .where("id", "=", userId)
      .forUpdate()
      .executeTakeFirst();
    if (!user?.deleted_at) return true;
    if (user.clerk_deleted_at) return false;
    await trx
      .updateTable("users")
      .set({
        deleted_at: null,
        purge_after: null,
        clerk_attempts: 0,
        clerk_retry_at: null,
        billing_attempts: 0,
        billing_retry_at: null,
      })
      .where("id", "=", userId)
      .execute();
    await audit(trx, {
      userId,
      actor: `user:${userId}`,
      action: "account.restored",
    });
    return true;
  });
}

async function auditClerkDeletion(
  trx: Kysely<Tables>,
  userId: string,
  actor: Actor,
) {
  await audit(trx, {
    userId,
    actor,
    action: "account.deleted_in_clerk",
  });
}

async function recordClerkDeletion(
  trx: Kysely<Tables>,
  userId: string,
  actor: Actor,
) {
  await trx
    .updateTable("users")
    .set({ clerk_deleted_at: sql<Date>`now()`, clerk_retry_at: null })
    .where("id", "=", userId)
    .execute();
  await auditClerkDeletion(trx, userId, actor);
}

/**
 * Asks Clerk to delete the user, unless another attempt holds the account or Clerk already did.
 * Returns whether the user is gone from Clerk. A failure is recorded for the sweeper to retry.
 */
export async function deleteInClerk(
  db: Kysely<Tables>,
  clerk: ClerkBackend | null,
  log: Logger,
  userId: string,
): Promise<boolean> {
  if (!clerk) {
    log.warn(
      { userId },
      "CLERK_SECRET_KEY is not set, so deleting this user in Clerk waits until it is",
    );
    return false;
  }
  return db.transaction().execute(async (trx) => {
    const user = await trx
      .selectFrom("users")
      .select(["clerk_deleted_at", "clerk_attempts"])
      .where("id", "=", userId)
      .where("deleted_at", "is not", null)
      .forUpdate()
      .skipLocked()
      .executeTakeFirst();
    if (!user) return false;
    if (user.clerk_deleted_at) return true;
    try {
      await clerk.deleteUser(userId);
    } catch (error) {
      await trx
        .updateTable("users")
        .set(
          clerkFailed(log, "delete a deleted account", {
            error,
            userId,
            attempts: user.clerk_attempts + 1,
          }),
        )
        .where("id", "=", userId)
        .execute();
      return false;
    }
    await recordClerkDeletion(trx, userId, "system");
    log.info({ userId }, "deleted the user in Clerk");
    return true;
  });
}

/**
 * Revokes the Clerk session that registered a phone removed from elsewhere, so the phone cannot
 * sign itself back in. A failure is recorded for the sweeper to retry.
 */
export async function revokeRemovedSession(
  db: Kysely<Tables>,
  clerk: ClerkBackend | null,
  log: Logger,
  key: string,
): Promise<void> {
  if (!clerk) {
    log.warn(
      { key: key.slice(0, 8) },
      "CLERK_SECRET_KEY is not set, so revoking the removed phone's session waits until it is",
    );
    return;
  }
  await db.transaction().execute(async (trx) => {
    const tombstone = await trx
      .selectFrom("removed_devices")
      .select(["clerk_session_id", "clerk_attempts"])
      .where("key", "=", key)
      .where("clerk_revoked_at", "is", null)
      .where("clerk_retry_at", "is not", null)
      .forUpdate()
      .skipLocked()
      .executeTakeFirst();
    if (!tombstone?.clerk_session_id) return;
    try {
      await clerk.revokeSession(tombstone.clerk_session_id);
    } catch (error) {
      await trx
        .updateTable("removed_devices")
        .set(
          clerkFailed(log, "revoke a removed phone's session", {
            error,
            key: key.slice(0, 8),
            attempts: tombstone.clerk_attempts + 1,
          }),
        )
        .where("key", "=", key)
        .execute();
      return;
    }
    await trx
      .updateTable("removed_devices")
      .set({ clerk_revoked_at: sql<Date>`now()`, clerk_retry_at: null })
      .where("key", "=", key)
      .execute();
    log.info({ key: key.slice(0, 8) }, "revoked a removed phone's session");
  });
}
