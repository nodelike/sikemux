import type { OrgRole } from "@sikemux/protocol";
import { sql, type Kysely, type Transaction } from "kysely";

import { audit } from "../audit.ts";
import { PLAN_PRODUCTS } from "../billing/catalog.ts";
import { checkSeatPauses } from "../billing/seat-pause.ts";
import type { Tables } from "../db.ts";
import { appendEvent, appendEvents } from "../events/log.ts";
import { membersOf, orgsChanged, seatsChanged } from "../orgs/shared.ts";
import { dropUnshared } from "../orgvault/projects.ts";
import { PURGE_AFTER_DAYS } from "./deletion.ts";

const ROLE_RANK: Record<OrgRole, number> = { user: 0, admin: 1, owner: 2 };

export interface BillingMergeConflict {
  provider: string;
  targetSubscriptionId: string;
  targetProviderSubscriptionId: string;
  sourceSubscriptionId: string;
  sourceProviderSubscriptionId: string;
}

export interface MergeResult {
  alreadyMerged: boolean;
  sourceWasDeleted: boolean;
  devices: number;
  orgs: number;
  pages: number;
  stash: number;
  vaultMoved: boolean;
  vaultConflict: boolean;
  billingConflicts: BillingMergeConflict[];
}

interface MergeRequest {
  sourceUserId: string;
  targetUserId: string;
  messageId: string;
}

export async function mergeAccount(
  db: Kysely<Tables>,
  { sourceUserId, targetUserId, messageId }: MergeRequest,
): Promise<MergeResult> {
  if (sourceUserId === targetUserId) {
    return {
      alreadyMerged: true,
      sourceWasDeleted: false,
      devices: 0,
      orgs: 0,
      pages: 0,
      stash: 0,
      vaultMoved: false,
      vaultConflict: false,
      billingConflicts: [],
    };
  }

  return db.transaction().execute(async (trx) => {
    for (const id of [sourceUserId, targetUserId].sort()) {
      await trx
        .insertInto("users")
        .values({ id })
        .onConflict((oc) => oc.column("id").doNothing())
        .execute();
    }

    const users = await trx
      .selectFrom("users")
      .selectAll()
      .where("id", "in", [sourceUserId, targetUserId])
      .orderBy("id")
      .forUpdate()
      .execute();

    const source = users.find((u) => u.id === sourceUserId);
    const target = users.find((u) => u.id === targetUserId);
    if (!source || !target)
      throw new Error("Expected both user rows to exist during merge.");

    const already = await trx
      .selectFrom("audit")
      .select("id")
      .where("subject", "=", sourceUserId)
      .where("action", "=", "webhook.user_merged")
      .executeTakeFirst();
    if (already) {
      return {
        alreadyMerged: true,
        sourceWasDeleted: source.deleted_at !== null,
        devices: 0,
        orgs: 0,
        pages: 0,
        stash: 0,
        vaultMoved: false,
        vaultConflict: false,
        billingConflicts: [],
      };
    }

    const sourceWasDeleted = source.deleted_at !== null;

    await mergeUserRow(trx, source, target);

    const movedDevices = await trx
      .updateTable("devices")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .returning(["key", "role"])
      .execute();

    await trx
      .updateTable("removed_devices")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .where("reason", "<>", "account_deleted")
      .execute();

    await trx
      .updateTable("challenges")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();

    await trx
      .updateTable("host_sign_ins")
      .set({ approved_by: targetUserId })
      .where("approved_by", "=", sourceUserId)
      .execute();

    await trx
      .updateTable("host_removal_sign_outs")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();

    await trx
      .updateTable("ended_sessions")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();

    await trx
      .updateTable("shares")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();

    const movedPages = await trx
      .updateTable("pages")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .returning("id")
      .execute();

    const movedStash = await trx
      .updateTable("stash_items")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .returning("id")
      .execute();

    await trx
      .updateTable("hooks")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();

    const { vaultMoved, modsMoved, vaultConflict } = await mergeVaultAndMods(
      trx,
      sourceUserId,
      targetUserId,
    );

    const orgsMoved = await mergeOrgs(trx, sourceUserId, targetUserId);

    const { billingChanged, billingConflicts } = await mergeBilling(
      trx,
      sourceUserId,
      targetUserId,
    );

    await mergeWorkAddressesAndSignIns(trx, sourceUserId, targetUserId);

    await trx
      .updateTable("audit")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();

    await trx
      .updateTable("audit")
      .set({ actor: `user:${targetUserId}` })
      .where("actor", "=", `user:${sourceUserId}`)
      .execute();

    const targetEvents: Parameters<typeof appendEvents>[1] = [
      ...movedDevices.map((device) => ({
        userId: targetUserId,
        type: "device.added" as const,
        subject: device.key,
        subjectRole: device.role,
      })),
      ...(movedStash[0]
        ? [
            {
              userId: targetUserId,
              type: "stash.added" as const,
              subject: movedStash[0].id,
            },
          ]
        : []),
      ...(vaultMoved
        ? [{ userId: targetUserId, type: "vault.changed" as const }]
        : []),
      ...(modsMoved
        ? [{ userId: targetUserId, type: "mods.changed" as const }]
        : []),
      ...(billingChanged && orgsMoved === 0
        ? [{ userId: targetUserId, type: "plan.changed" as const }]
        : []),
    ];
    if (targetEvents.length) await appendEvents(trx, targetEvents);
    if (billingChanged) await checkSeatPauses(trx, [targetUserId]);

    await trx
      .updateTable("users")
      .set({
        deleted_at: sql<Date>`coalesce(deleted_at, now())`,
        clerk_deleted_at: sql<Date>`coalesce(clerk_deleted_at, now())`,
        purge_after: sql<Date>`coalesce(purge_after, now() + make_interval(days => ${PURGE_AFTER_DAYS}))`,
        clerk_attempts: 0,
        clerk_retry_at: null,
        billing_attempts: 0,
        billing_retry_at: billingConflicts.length > 0 ? sql<Date>`now()` : null,
      })
      .where("id", "=", sourceUserId)
      .execute();

    if (!sourceWasDeleted)
      await appendEvent(trx, {
        userId: sourceUserId,
        type: "account.deleted",
      });

    await audit(trx, {
      userId: targetUserId,
      actor: "clerk",
      action: "webhook.user_merged",
      subject: sourceUserId,
      detail: {
        messageId,
        devices: movedDevices.length,
        orgs: orgsMoved,
        pages: movedPages.length,
        stash: movedStash.length,
        vaultMoved,
        vaultConflict,
        billingConflicts: billingConflicts.length,
      },
    });

    return {
      alreadyMerged: false,
      sourceWasDeleted,
      devices: movedDevices.length,
      orgs: orgsMoved,
      pages: movedPages.length,
      stash: movedStash.length,
      vaultMoved,
      vaultConflict,
      billingConflicts,
    };
  });
}

async function mergeUserRow(
  trx: Transaction<Tables>,
  source: {
    staff: boolean;
    admitted_at: Date | null;
    new_kept_at: Date | null;
    stash_seen_at: Date | null;
    frozen_at: Date | null;
  },
  target: {
    id: string;
    staff: boolean;
    admitted_at: Date | null;
    new_kept_at: Date | null;
    stash_seen_at: Date | null;
    frozen_at: Date | null;
  },
) {
  const stashSeenAt =
    target.stash_seen_at && source.stash_seen_at
      ? target.stash_seen_at >= source.stash_seen_at
        ? target.stash_seen_at
        : source.stash_seen_at
      : (target.stash_seen_at ?? source.stash_seen_at);

  const patch: {
    staff?: boolean;
    admitted_at?: Date;
    new_kept_at?: Date;
    stash_seen_at?: Date;
    frozen_at?: Date;
  } = {};

  if (source.staff && !target.staff) patch.staff = true;
  if (!target.admitted_at && source.admitted_at)
    patch.admitted_at = source.admitted_at;
  if (!target.new_kept_at && source.new_kept_at)
    patch.new_kept_at = source.new_kept_at;
  if (stashSeenAt && stashSeenAt.getTime() !== target.stash_seen_at?.getTime())
    patch.stash_seen_at = stashSeenAt;
  if (!target.frozen_at && source.frozen_at) patch.frozen_at = source.frozen_at;

  if (Object.keys(patch).length > 0) {
    await trx
      .updateTable("users")
      .set(patch)
      .where("id", "=", target.id)
      .execute();
  }
}

async function mergeVaultAndMods(
  trx: Transaction<Tables>,
  sourceUserId: string,
  targetUserId: string,
): Promise<{
  vaultMoved: boolean;
  modsMoved: boolean;
  vaultConflict: boolean;
}> {
  const [sourceVault, targetVault] = await Promise.all([
    trx
      .selectFrom("vaults")
      .select("user_id")
      .where("user_id", "=", sourceUserId)
      .forUpdate()
      .executeTakeFirst(),
    trx
      .selectFrom("vaults")
      .select("user_id")
      .where("user_id", "=", targetUserId)
      .forUpdate()
      .executeTakeFirst(),
  ]);

  if (!sourceVault) {
    await trx
      .updateTable("vault_uses")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();
    return { vaultMoved: false, modsMoved: false, vaultConflict: false };
  }

  // Personal vaults and mods are end-to-end encrypted under distinct per-account keys.
  if (targetVault) {
    return { vaultMoved: false, modsMoved: false, vaultConflict: true };
  }

  await sql`
    insert into vaults (user_id, generation, recovery_key, recovery_wrap, rotate, created_by, created_at, rotated_at)
    select ${targetUserId}, generation, recovery_key, recovery_wrap, rotate, created_by, created_at, rotated_at
    from vaults
    where user_id = ${sourceUserId}`.execute(trx);

  const sourceModCounter = await trx
    .selectFrom("mod_counters")
    .select("user_id")
    .where("user_id", "=", sourceUserId)
    .forUpdate()
    .executeTakeFirst();

  if (sourceModCounter) {
    await sql`
      insert into mod_counters (user_id, seq, sealed_bytes, cleared_seq, file_bytes)
      select ${targetUserId}, seq, sealed_bytes, cleared_seq, file_bytes
      from mod_counters
      where user_id = ${sourceUserId}`.execute(trx);
  }

  await trx
    .updateTable("vault_keys")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();
  await trx
    .updateTable("vault_entries")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();
  await trx
    .updateTable("vault_ended")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();
  await trx
    .updateTable("vault_uses")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();

  if (sourceModCounter) {
    await trx
      .updateTable("mod_documents")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();
    await trx
      .updateTable("mod_purges")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();
    await trx
      .updateTable("mod_files")
      .set({ user_id: targetUserId })
      .where("user_id", "=", sourceUserId)
      .execute();
    await trx
      .deleteFrom("mod_counters")
      .where("user_id", "=", sourceUserId)
      .execute();
  }

  await trx.deleteFrom("vaults").where("user_id", "=", sourceUserId).execute();

  return {
    vaultMoved: true,
    modsMoved: sourceModCounter !== undefined,
    vaultConflict: false,
  };
}

async function mergeOrgs(
  trx: Transaction<Tables>,
  sourceUserId: string,
  targetUserId: string,
): Promise<number> {
  await trx
    .updateTable("billing_customers")
    .set({ paid_by: targetUserId })
    .where("paid_by", "=", sourceUserId)
    .execute();

  await trx
    .updateTable("orgs")
    .set({ created_by: targetUserId })
    .where("created_by", "=", sourceUserId)
    .execute();

  await trx
    .updateTable("org_users")
    .set({ invited_by: targetUserId })
    .where("invited_by", "=", sourceUserId)
    .execute();

  await trx
    .updateTable("org_invites")
    .set({ invited_by: targetUserId })
    .where("invited_by", "=", sourceUserId)
    .execute();

  await trx
    .updateTable("org_invites")
    .set({ accepted_by: targetUserId })
    .where("accepted_by", "=", sourceUserId)
    .execute();

  await trx
    .updateTable("org_domains")
    .set({ added_by: targetUserId })
    .where("added_by", "=", sourceUserId)
    .execute();

  const orgs = await trx
    .selectFrom("org_users")
    .select("org_id")
    .distinct()
    .where("user_id", "=", sourceUserId)
    .orderBy("org_id")
    .execute();

  const notifyOrgs = new Set<string>();
  const shareChangedOrgs = new Set<string>();
  let count = 0;

  for (const { org_id: orgId } of orgs) {
    await trx
      .selectFrom("orgs")
      .select("id")
      .where("id", "=", orgId)
      .forUpdate()
      .execute();

    const seat = await trx
      .selectFrom("org_users")
      .select(["id", "role"])
      .where("org_id", "=", orgId)
      .where("user_id", "=", sourceUserId)
      .executeTakeFirst();
    if (!seat) continue;

    const kept = await trx
      .selectFrom("org_users")
      .select(["id", "role"])
      .where("org_id", "=", orgId)
      .where("user_id", "=", targetUserId)
      .executeTakeFirst();

    let role: OrgRole = seat.role;
    if (!kept) {
      const doubled = await trx
        .deleteFrom("org_shares")
        .where("member_id", "=", seat.id)
        .where("project", "in", (eb) =>
          eb
            .selectFrom("org_shares")
            .select("project")
            .where("user_id", "=", targetUserId),
        )
        .executeTakeFirst();
      await trx
        .updateTable("org_users")
        .set({ user_id: targetUserId })
        .where("id", "=", seat.id)
        .execute();
      if (doubled.numDeletedRows > 0n) {
        await dropUnshared(trx, orgId);
        shareChangedOrgs.add(orgId);
      }
    } else {
      if (ROLE_RANK[seat.role] > ROLE_RANK[kept.role]) {
        await trx
          .updateTable("org_users")
          .set({ role: seat.role })
          .where("id", "=", kept.id)
          .execute();
      } else {
        role = kept.role;
      }

      await sql`
        insert into org_team_members (team_id, member_id)
        select team_id, ${kept.id} from org_team_members where member_id = ${seat.id}
        on conflict do nothing`.execute(trx);

      const movedShares = await trx
        .updateTable("org_shares")
        .set({ member_id: kept.id, user_id: targetUserId })
        .where("member_id", "=", seat.id)
        .where("project", "not in", (eb) =>
          eb
            .selectFrom("org_shares")
            .select("project")
            .where("user_id", "=", targetUserId),
        )
        .returning("project")
        .execute();

      const remainingShares = await trx
        .selectFrom("org_shares")
        .select("project")
        .where("member_id", "=", seat.id)
        .limit(1)
        .executeTakeFirst();

      await trx.deleteFrom("org_users").where("id", "=", seat.id).execute();
      await dropUnshared(trx, orgId);
      if (movedShares.length > 0 || remainingShares !== undefined)
        shareChangedOrgs.add(orgId);
    }

    notifyOrgs.add(orgId);
    count += 1;

    await audit(trx, {
      userId: targetUserId,
      actor: "clerk",
      action: "org.seat_moved",
      subject: orgId,
      detail: { role, merged: kept !== undefined },
    });
  }

  for (const orgId of notifyOrgs)
    await orgsChanged(trx, await membersOf(trx, orgId));

  for (const orgId of shareChangedOrgs) {
    const members = await membersOf(trx, orgId);
    await appendEvents(
      trx,
      members.map((userId) => ({ userId, type: "shares.changed" as const })),
    );
  }

  if (count > 0) await seatsChanged(trx, [targetUserId]);

  return count;
}

async function mergeBilling(
  trx: Transaction<Tables>,
  sourceUserId: string,
  targetUserId: string,
): Promise<{
  billingChanged: boolean;
  billingConflicts: BillingMergeConflict[];
}> {
  let billingChanged = false;
  const billingConflicts: BillingMergeConflict[] = [];

  await trx
    .updateTable("billing_checkouts")
    .set({ opened_by: targetUserId })
    .where("opened_by", "=", sourceUserId)
    .execute();

  await trx
    .updateTable("billing_calls")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();

  const sourceGift = await trx
    .selectFrom("plan_gifts")
    .selectAll()
    .where("user_id", "=", sourceUserId)
    .executeTakeFirst();
  if (sourceGift) {
    await sql`
      insert into plan_gifts (user_id, plan, given_at, ends_at)
      values (${targetUserId}, ${sourceGift.plan}, ${sourceGift.given_at}, ${sourceGift.ends_at})
      on conflict (user_id) do update
        set given_at = least(plan_gifts.given_at, excluded.given_at),
            ends_at = greatest(plan_gifts.ends_at, excluded.ends_at)`.execute(
      trx,
    );
    await trx
      .deleteFrom("plan_gifts")
      .where("user_id", "=", sourceUserId)
      .execute();
    billingChanged = true;
  }

  const customers = await trx
    .selectFrom("billing_customers")
    .selectAll()
    .where("user_id", "in", [sourceUserId, targetUserId])
    .orderBy("id")
    .forUpdate()
    .execute();

  const sourceCustomers = customers.filter((c) => c.user_id === sourceUserId);
  const targetByProvider = new Map(
    customers
      .filter((c) => c.user_id === targetUserId)
      .map((c) => [c.provider, c]),
  );
  const conflictProviders = new Set<string>();

  for (const srcCust of sourceCustomers) {
    const dstCust = targetByProvider.get(srcCust.provider);
    if (!dstCust) {
      await trx
        .updateTable("billing_customers")
        .set({ user_id: targetUserId })
        .where("id", "=", srcCust.id)
        .execute();
      billingChanged = true;
      continue;
    }

    const [srcOpenPlan, dstOpenPlan] = await Promise.all([
      trx
        .selectFrom("subscriptions")
        .select(["id", "provider_subscription_id"])
        .where("customer_id", "=", srcCust.id)
        .where("status", "<>", "ended")
        .where("product", "in", PLAN_PRODUCTS)
        .executeTakeFirst(),
      trx
        .selectFrom("subscriptions")
        .select(["id", "provider_subscription_id"])
        .where("customer_id", "=", dstCust.id)
        .where("status", "<>", "ended")
        .where("product", "in", PLAN_PRODUCTS)
        .executeTakeFirst(),
    ]);

    if (srcOpenPlan && dstOpenPlan) {
      conflictProviders.add(srcCust.provider);
      billingConflicts.push({
        provider: srcCust.provider,
        targetSubscriptionId: dstOpenPlan.id,
        targetProviderSubscriptionId: dstOpenPlan.provider_subscription_id,
        sourceSubscriptionId: srcOpenPlan.id,
        sourceProviderSubscriptionId: srcOpenPlan.provider_subscription_id,
      });
      continue;
    }

    if (srcOpenPlan && !dstOpenPlan) {
      await repointCustomerChildren(trx, dstCust.id, srcCust.id);
      await trx
        .deleteFrom("billing_customers")
        .where("id", "=", dstCust.id)
        .execute();
      await trx
        .updateTable("billing_customers")
        .set({ user_id: targetUserId })
        .where("id", "=", srcCust.id)
        .execute();
      await trx
        .deleteFrom("billing_people")
        .where("provider", "=", srcCust.provider)
        .where("user_id", "=", targetUserId)
        .execute();
      await trx
        .updateTable("billing_people")
        .set({ user_id: targetUserId })
        .where("provider", "=", srcCust.provider)
        .where("user_id", "=", sourceUserId)
        .execute();
      billingChanged = true;
      continue;
    }

    await repointCustomerChildren(trx, srcCust.id, dstCust.id);
    if (!dstCust.provider_customer_id && srcCust.provider_customer_id) {
      await trx
        .updateTable("billing_customers")
        .set({ provider_customer_id: srcCust.provider_customer_id })
        .where("id", "=", dstCust.id)
        .execute();
    }
    await trx
      .deleteFrom("billing_customers")
      .where("id", "=", srcCust.id)
      .execute();
    billingChanged = true;
  }

  const remainingPeople = await trx
    .selectFrom("billing_people")
    .select(["provider", "provider_customer_id"])
    .where("user_id", "=", sourceUserId)
    .execute();

  for (const person of remainingPeople) {
    if (conflictProviders.has(person.provider)) continue;
    const targetPerson = await trx
      .selectFrom("billing_people")
      .select("provider_customer_id")
      .where("provider", "=", person.provider)
      .where("user_id", "=", targetUserId)
      .executeTakeFirst();
    if (!targetPerson) {
      await trx
        .updateTable("billing_people")
        .set({ user_id: targetUserId })
        .where("provider", "=", person.provider)
        .where("user_id", "=", sourceUserId)
        .execute();
    } else {
      await trx
        .deleteFrom("billing_people")
        .where("provider", "=", person.provider)
        .where("user_id", "=", sourceUserId)
        .execute();
    }
  }

  return { billingChanged, billingConflicts };
}

async function repointCustomerChildren(
  trx: Transaction<Tables>,
  fromCustomerId: string,
  toCustomerId: string,
) {
  await trx
    .updateTable("subscriptions")
    .set({ customer_id: toCustomerId })
    .where("customer_id", "=", fromCustomerId)
    .execute();
  await trx
    .updateTable("invoices")
    .set({ customer_id: toCustomerId })
    .where("customer_id", "=", fromCustomerId)
    .execute();
  await trx
    .updateTable("billing_checkouts")
    .set({ customer_id: toCustomerId })
    .where("customer_id", "=", fromCustomerId)
    .execute();
}

async function mergeWorkAddressesAndSignIns(
  trx: Transaction<Tables>,
  sourceUserId: string,
  targetUserId: string,
) {
  await trx
    .updateTable("address_proofs")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();

  await trx
    .updateTable("work_address_sittings")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();

  await sql`
    update addresses_seen as dst
    set first_seen = least(dst.first_seen, src.first_seen),
        could_sign_in = dst.could_sign_in or src.could_sign_in
    from addresses_seen as src
    where dst.user_id = ${targetUserId}
      and src.user_id = ${sourceUserId}
      and dst.ended_at is null
      and src.ended_at is null
      and dst.address_hash = src.address_hash`.execute(trx);

  await sql`
    update sign_in_removals as r
    set address_seen_id = dst.id
    from addresses_seen as src
    join addresses_seen as dst
      on dst.user_id = ${targetUserId}
     and dst.ended_at is null
     and dst.address_hash = src.address_hash
    where r.address_seen_id = src.id
      and src.user_id = ${sourceUserId}
      and src.ended_at is null`.execute(trx);

  await sql`
    delete from addresses_seen as src
    using addresses_seen as dst
    where src.user_id = ${sourceUserId}
      and dst.user_id = ${targetUserId}
      and src.ended_at is null
      and dst.ended_at is null
      and src.address_hash = dst.address_hash`.execute(trx);

  await trx
    .updateTable("addresses_seen")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();

  await sql`
    update sign_in_removals as dst
    set end_sessions_from = least(dst.end_sessions_from, src.end_sessions_from),
        swept_from = least(dst.swept_from, src.swept_from),
        told_at = coalesce(dst.told_at, src.told_at)
    from sign_in_removals as src
    where dst.user_id = ${targetUserId}
      and src.user_id = ${sourceUserId}
      and dst.address_id = src.address_id`.execute(trx);

  await sql`
    delete from sign_in_removals as src
    using sign_in_removals as dst
    where src.user_id = ${sourceUserId}
      and dst.user_id = ${targetUserId}
      and src.address_id = dst.address_id`.execute(trx);

  await trx
    .updateTable("sign_in_removals")
    .set({ user_id: targetUserId })
    .where("user_id", "=", sourceUserId)
    .execute();

  await trx
    .updateTable("account_folds")
    .set({ expires_at: sql<Date>`least(expires_at, now())` })
    .where("done_at", "is", null)
    .where((eb) =>
      eb.or([
        eb("from_user", "=", sourceUserId),
        eb("into_user", "=", sourceUserId),
      ]),
    )
    .execute();

  await trx
    .updateTable("account_folds")
    .set({ into_user: targetUserId })
    .where("done_at", "is not", null)
    .where("into_user", "=", sourceUserId)
    .where((eb) =>
      eb.or([eb("from_user", "is", null), eb("from_user", "<>", targetUserId)]),
    )
    .execute();
}
