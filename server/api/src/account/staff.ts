import type { Kysely } from "kysely";

import type { Tables } from "../db.ts";

/** Marked by hand in the database: `update users set staff = true where id = '<Clerk user id>'`. */
export async function isStaff(db: Kysely<Tables>, userId: string) {
  const user = await db
    .selectFrom("users")
    .select("staff")
    .where("id", "=", userId)
    .executeTakeFirst();
  return user?.staff ?? false;
}
