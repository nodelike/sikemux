import type { Services } from "../app.ts";
import type { Allowance } from "@sikemux/protocol";
import { Hono } from "hono";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import { perUser } from "../limits.ts";
import { allowance } from "./allowance.ts";

/** GET /v1/account/allowance: what the account a host is on may use, read again on plan.changed. */
export function allowanceRoutes({
  database: { db },
  verifier,
  limiter,
  billing: { live },
}: Pick<Services, "database" | "verifier" | "limiter" | "billing">) {
  return new Hono<AuthEnv>().get(
    "/",
    requireIdentity(verifier, db, { accept: ["host"] }),
    perUser(limiter, "allowance", 30),
    async (c) => {
      const body: Allowance = await allowance(
        db,
        { userId: c.get("identity").userId },
        { live },
      );
      return c.json(body);
    },
  );
}
