import type { Transaction } from "kysely";

import type { Tables } from "../db.ts";
import { appendEvent } from "../events/log.ts";

/** A host left the account: its copy of the vault key goes, and the key it knew must be replaced. */
export async function leaveVault(
  trx: Transaction<Tables>,
  userId: string,
  hostKey: string,
): Promise<void> {
  await trx
    .deleteFrom("vault_keys")
    .where("user_id", "=", userId)
    .where("host_key", "=", hostKey)
    .execute();
  const marked = await trx
    .updateTable("vaults")
    .set({ rotate: true })
    .where("user_id", "=", userId)
    .executeTakeFirst();
  if (marked.numUpdatedRows > 0n)
    await appendEvent(trx, { userId, type: "vault.changed" });
}
