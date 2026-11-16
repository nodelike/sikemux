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

/** The orgs the person is the only owner of, which must be handed over or deleted before they go. */
export async function orgsOnlyOwnedBy(
  db: Kysely<Tables>,
  userId: string,
): Promise<string[]> {
  const rows = await db
    .selectFrom("org_users as mine")
    .select("mine.org_id")
    .where("mine.user_id", "=", userId)
    .where("mine.role", "=", "owner")
    .where((eb) =>
      eb.not(
        eb.exists(
          eb
            .selectFrom("org_users as other")
            .select("other.user_id")
            .whereRef("other.org_id", "=", "mine.org_id")
            .where("other.role", "=", "owner")
            .where("other.user_id", "<>", userId),
        ),
      ),
    )
    .execute();
  return rows.map((row) => row.org_id);
}
