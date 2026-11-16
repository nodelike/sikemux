import type { MiddlewareHandler } from "hono";
import type { Kysely } from "kysely";

import { isStaff } from "../account/staff.ts";
import type { AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { notFound } from "../http.ts";

/**
 * After requireIdentity: lets through only staff accounts, until plans go live. Everyone else
 * hears the route does not exist, as for any unknown path.
 */
export function requireBillingAccess(
  db: Kysely<Tables>,
): MiddlewareHandler<AuthEnv> {
  return async (c, next) => {
    if (!(await isStaff(db, c.get("identity").userId)))
      throw notFound(`No route matches ${c.req.method} ${c.req.path}.`);
    await next();
  };
}
