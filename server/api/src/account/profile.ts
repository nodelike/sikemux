import type { Services } from "../app.ts";
import type { AccountProfile } from "@sikemux/protocol";
import { Hono } from "hono";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import { ApiFailure } from "../http.ts";
import { perUser } from "../limits.ts";
import { isStaff } from "./staff.ts";

/** GET /v1/account/profile: who a host belongs to, for the host to show, read from Clerk. */
export function profileRoutes({
  database: { db },
  verifier,
  limiter,
  clerk,
}: Pick<Services, "database" | "verifier" | "limiter" | "clerk">) {
  return new Hono<AuthEnv>().get(
    "/",
    requireIdentity(verifier, db, { accept: ["host"] }),
    perUser(limiter, "profile", 10),
    async (c) => {
      if (!clerk)
        throw new ApiFailure(
          503,
          "unavailable",
          "The account's profile cannot be read right now.",
        );
      const { userId } = c.get("identity");
      const [profile, staff] = await Promise.all([
        clerk.profile(userId),
        isStaff(db, userId),
      ]);
      const body: AccountProfile = { ...profile, staff };
      return c.json(body);
    },
  );
}
