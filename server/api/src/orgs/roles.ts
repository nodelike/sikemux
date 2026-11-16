import type { OrgRole } from "@sikemux/protocol";
import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";

/** The person's role in the org, or null when they are not in it. */
export async function orgRole(
  db: Kysely<Tables>,
  orgId: string,
  userId: string,
): Promise<OrgRole | null> {
  const member = await db
    .selectFrom("org_users")
    .select("role")
    .where("org_id", "=", orgId)
    .where("user_id", "=", userId)
    .executeTakeFirst();
  return member?.role ?? null;
}

/** Whether a member with `actor`'s role may change, remove or invite someone with `role`. */
export function manages(actor: OrgRole, role: OrgRole): boolean {
  return actor === "owner" || (actor === "admin" && role !== "owner");
}
