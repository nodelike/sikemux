import type {
  OrgVaultVouchPut,
  OrgVaultVouches,
  OrgWrappedKey,
} from "@sikemux/protocol";
import type { Transaction } from "kysely";

import { mayHoldVaults, VAULT_SOON } from "../billing/team.ts";
import type { Tables } from "../db.ts";
import { badRequest, forbidden, type ApiFailure } from "../http.ts";
import {
  auditOrg,
  lockedVault,
  requireHolder,
  vaultChanged,
  type Asker,
} from "./change.ts";
import { memberHosts, type Member } from "./state.ts";

/** Those of `keys` for a host that some host vouched for under the membership it has now. */
export async function vouchedKeys(
  trx: Transaction<Tables>,
  orgId: string,
  keys: OrgWrappedKey[],
): Promise<OrgWrappedKey[]> {
  if (!keys.length) return [];
  const rows = await trx
    .selectFrom("org_vault_vouches")
    .innerJoin("devices", "devices.key", "org_vault_vouches.host_key")
    .innerJoin("org_users", (join) =>
      join
        .onRef("org_users.user_id", "=", "devices.user_id")
        .onRef("org_users.org_id", "=", "org_vault_vouches.org_id")
        .onRef("org_users.id", "=", "org_vault_vouches.member"),
    )
    .select("org_vault_vouches.host_key")
    .distinct()
    .where("org_vault_vouches.org_id", "=", orgId)
    .where(
      "org_vault_vouches.host_key",
      "in",
      keys.map((key) => key.hostKey),
    )
    .execute();
  const vouched = new Set(rows.map((row) => row.host_key));
  return keys.filter((key) => vouched.has(key.hostKey));
}

/**
 * Which of `vouches` the asking host may sign, with the membership each is for, and why the first
 * of the rest is refused.
 */
async function mayVouch(
  trx: Transaction<Tables>,
  { orgId, userId, hostKey }: Asker,
  { id, role, active }: Member,
  vouches: OrgVaultVouchPut[],
) {
  const wanted = vouches.map((vouch) => vouch.hostKey);
  if (!active)
    throw forbidden(
      "The org's Team plan has lapsed, or isn't open to your account now, so no computer is let into its vault.",
    );
  if (wanted.includes(hostKey) || new Set(wanted).size !== wanted.length)
    throw badRequest("A host vouches once for each other host.");
  const hosts = await memberHosts(trx, orgId)
    .select((eb) => mayHoldVaults(eb).as("open"))
    .where("devices.key", "in", wanted)
    .execute();
  const listed = new Map(hosts.map((host) => [host.key, host]));
  const letInBy = new Set(
    (
      await trx
        .selectFrom("org_vault_vouches")
        .innerJoin("devices", "devices.key", "org_vault_vouches.by_key")
        .innerJoin("org_users", (join) =>
          join
            .onRef("org_users.user_id", "=", "devices.user_id")
            .onRef("org_users.org_id", "=", "org_vault_vouches.org_id")
            .onRef("org_users.id", "=", "org_vault_vouches.by_member"),
        )
        .select("org_vault_vouches.by_key")
        .where("org_vault_vouches.org_id", "=", orgId)
        .where("org_vault_vouches.host_key", "=", hostKey)
        .where("org_vault_vouches.member", "=", id)
        .execute()
    ).map((vouch) => vouch.by_key),
  );
  const refusal = (key: string): ApiFailure | undefined => {
    const host = listed.get(key);
    if (!host)
      return badRequest("A vouch is for a device that is not a member's host.");
    if (!host.open) return forbidden(VAULT_SOON);
    if (role === "user" && host.user_id !== userId && !letInBy.has(key))
      return forbidden(
        "Only an org's owner or admin lets another account's host into its vault.",
      );
    return undefined;
  };
  const allowed = vouches.flatMap((vouch) => {
    const host = listed.get(vouch.hostKey);
    return host && !refusal(vouch.hostKey) ? [{ vouch, host }] : [];
  });
  return { allowed, refused: wanted.map(refusal).find(Boolean) };
}

async function insertVouches(
  trx: Transaction<Tables>,
  asker: Asker,
  member: Member,
  allowed: Awaited<ReturnType<typeof mayVouch>>["allowed"],
): Promise<boolean> {
  if (!allowed.length) return false;
  const kept = await trx
    .insertInto("org_vault_vouches")
    .values(
      allowed.map(({ vouch, host }) => ({
        org_id: asker.orgId,
        host_key: vouch.hostKey,
        member: host.member,
        by_key: asker.hostKey,
        by_member: member.id,
        at: vouch.at,
        signature: vouch.signature,
        introduction: vouch.introduction ?? null,
      })),
    )
    .onConflict((oc) =>
      oc
        .columns(["org_id", "host_key", "by_key"])
        .doUpdateSet((eb) => ({
          member: eb.ref("excluded.member"),
          by_member: eb.ref("excluded.by_member"),
          at: eb.ref("excluded.at"),
          signature: eb.ref("excluded.signature"),
          introduction: eb
            .case()
            .when("excluded.introduction", "is not", null)
            .then(eb.ref("excluded.introduction"))
            .when(
              eb.and([
                eb("org_vault_vouches.member", "=", eb.ref("excluded.member")),
                eb(
                  "org_vault_vouches.by_member",
                  "=",
                  eb.ref("excluded.by_member"),
                ),
              ]),
            )
            .then(eb.ref("org_vault_vouches.introduction"))
            .else(null)
            .end(),
        }))
        .where((eb) =>
          eb.or([
            eb("excluded.introduction", "is not", null),
            eb(
              "org_vault_vouches.signature",
              "!=",
              eb.ref("excluded.signature"),
            ),
          ]),
        ),
    )
    .returning("host_key")
    .execute();
  const keptKeys = new Set(kept.map((row) => row.host_key));
  for (const { vouch, host } of allowed) {
    if (!keptKeys.has(vouch.hostKey)) continue;
    if (vouch.introduction)
      await auditOrg(trx, asker, "org_vault.host_let_in", {
        host: host.key,
        member: host.user_id,
        computer: host.name,
      });
    else
      await auditOrg(trx, asker, "org_vault.host_vouched", { host: host.key });
  }
  return kept.length > 0;
}

/** Keeps what the asking host signed of the hosts it lets in, or refuses the whole change. */
export async function letIn(
  trx: Transaction<Tables>,
  asker: Asker,
  member: Member,
  vouches: OrgVaultVouchPut[],
): Promise<void> {
  if (!vouches.length) return;
  const { allowed, refused } = await mayVouch(trx, asker, member, vouches);
  if (refused) throw refused;
  await insertVouches(trx, asker, member, allowed);
}

/** Keeps what the asking host may sign of other hosts and leaves out the rest, such as a host that just left. */
export async function putOrgVouches(
  trx: Transaction<Tables>,
  asker: Asker,
  member: Member,
  { vouches }: OrgVaultVouches,
): Promise<void> {
  const { generation } = await lockedVault(trx, asker.orgId);
  await requireHolder(trx, asker, generation);
  const { allowed } = await mayVouch(trx, asker, member, vouches);
  if (await insertVouches(trx, asker, member, allowed))
    await vaultChanged(trx, asker.orgId);
}
