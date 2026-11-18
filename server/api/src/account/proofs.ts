import { createHmac } from "node:crypto";
import { sql, type Kysely } from "kysely";

import type { Tables } from "../db.ts";

/** The keyed hash a proven work address is kept as, so the address itself is never stored. */
export function addressHash(key: Buffer, address: string): Buffer {
  return createHmac("sha256", key)
    .update(address.trim().toLowerCase())
    .digest();
}

export function domainOf(address: string): string | null {
  const at = address.lastIndexOf("@");
  return at > 0
    ? address
        .slice(at + 1)
        .trim()
        .toLowerCase()
    : null;
}

/**
 * A seat's end: the member's proofs made for that org, and the ones at a domain it has proven,
 * stop counting. Called inside the transaction that ends the seat, before the org is gone.
 */
export async function dropSeatProofs(
  trx: Kysely<Tables>,
  orgId: string,
  userIds: readonly string[],
): Promise<void> {
  if (userIds.length === 0) return;
  await sql`
    delete from address_proofs
    where user_id in (${sql.join(userIds)})
      and (
        org_id = ${orgId}
        or domain in (
          select domain from org_domains where org_id = ${orgId} and verified_at is not null
        )
      )`.execute(trx);
}
