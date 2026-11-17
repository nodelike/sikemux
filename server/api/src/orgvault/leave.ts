import type { Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { vaultChanged } from "./change.ts";

/**
 * A host left its account. Its wraps stay, marking each org vault it held for a new key, and
 * every member's hosts hear of it so an owner's or admin's host replaces the key.
 */
export async function leaveOrgVaults(
  trx: Transaction<Tables>,
  hostKey: string,
): Promise<void> {
  const held = await trx
    .selectFrom("org_vault_keys")
    .select("org_id")
    .where("host_key", "=", hostKey)
    .execute();
  for (const { org_id } of held) await vaultChanged(trx, org_id);
}
