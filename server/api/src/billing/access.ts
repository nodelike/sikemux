import type { PlanFeature } from "@sikemux/protocol";
import type { MiddlewareHandler } from "hono";
import type { Kysely } from "kysely";

import { isStaff } from "../account/staff.ts";
import type { AuthEnv } from "../auth.ts";
import type { Tables } from "../db.ts";
import { ApiFailure, notFound } from "../http.ts";
import { allowance } from "./allowance.ts";

/**
 * After requireIdentity: lets through only staff accounts until plans go live. Everyone else
 * hears the route does not exist, as for any unknown path.
 */
export function requireBillingAccess(
  db: Kysely<Tables>,
  live: boolean,
): MiddlewareHandler<AuthEnv> {
  return async (c, next) => {
    if (!live && !(await isStaff(db, c.get("identity").userId)))
      throw notFound(`No route matches ${c.req.method} ${c.req.path}.`);
    await next();
  };
}

/**
 * After requireIdentity: once plans are live, refuses an account whose plan lacks `feature`.
 * What the account already keeps stays.
 */
export function requireFeature(
  db: Kysely<Tables>,
  live: boolean,
  feature: PlanFeature,
): MiddlewareHandler<AuthEnv> {
  return async (c, next) => {
    if (live) {
      const { userId } = c.get("identity");
      const { features } = await allowance(db, { userId }, { live });
      if (!features.includes(feature))
        throw new ApiFailure(403, "forbidden", "Syncing needs Pro.");
    }
    await next();
  };
}
