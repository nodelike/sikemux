import type { Account, AccountDeletion, PushApp } from "@sikemux/protocol";
import { Hono } from "hono";

import {
  isWeb,
  requireIdentity,
  requireSession,
  type AuthEnv,
  type Verifier,
} from "../auth.ts";
import type { Database } from "../db.ts";
import { ApiFailure, readOptionalBody } from "../http.ts";
import { limit, type RateLimiter } from "../limits.ts";
import { revokeAppleSignIn, type AppleSignIn } from "./apple.ts";
import type { ClerkBackend } from "./clerk.ts";
import { markDeleted, PURGE_AFTER_DAYS, restoreAccount } from "./deletion.ts";

/** How recently the person must have proved their first factor to delete their account. */
const REVERIFY_MINUTES = 10;

export function accountRoutes(
  { db }: Database,
  verifier: Verifier,
  limiter: RateLimiter,
  clerk: ClerkBackend | null,
  apple: AppleSignIn | null,
  app: PushApp,
) {
  return new Hono<AuthEnv>()
    .use(requireIdentity(verifier, db, { allowDeleted: true }))
    .use(requireSession())
    .get("/", async (c) => {
      const { userId } = c.get("identity");
      const user = await db
        .selectFrom("users")
        .select(["deleted_at", "purge_after"])
        .where("id", "=", userId)
        .executeTakeFirst();
      const body: Account =
        user?.deleted_at && user.purge_after
          ? {
              status: "deleting",
              deletesAt: user.purge_after.toISOString(),
              restorableDays: PURGE_AFTER_DAYS,
            }
          : { status: "active", restorableDays: PURGE_AFTER_DAYS };
      return c.json(body);
    })
    .post(
      "/restore",
      limit<AuthEnv>(
        limiter,
        "restore-account",
        10,
        (c) => c.get("identity").userId,
      ),
      async (c) => {
        const identity = c.get("identity");
        if (!(await restoreAccount(db, identity.userId)))
          throw new ApiFailure(
            404,
            "not_found",
            "The account is already deleted and can no longer be restored.",
          );
        c.get("log").info("restored an account");
        const body: Account = {
          status: "active",
          restorableDays: PURGE_AFTER_DAYS,
        };
        return c.json(body);
      },
    )
    .delete(
      "/",
      limit<AuthEnv>(
        limiter,
        "delete-account",
        10,
        (c) => c.get("identity").userId,
      ),
      async (c) => {
        const identity = c.get("identity");
        if (
          identity.factorAgeMinutes === undefined ||
          identity.factorAgeMinutes > REVERIFY_MINUTES
        )
          throw new ApiFailure(403, "reverify_required", "reverify", {
            reverifyWithinMinutes: REVERIFY_MINUTES,
          });
        const request = await readOptionalBody(c, "AccountDeletionRequest");

        const deletion = await markDeleted(db, {
          userId: identity.userId,
          actor: `user:${identity.userId}`,
          via: isWeb(identity) ? "web" : "phone",
          deletedInClerk: false,
        });
        const log = c.get("log");
        log.info({ at: deletion.requestedAt }, "deleted an account");
        await revokeAppleSignIn(db, apple, clerk, log, {
          userId: identity.userId,
          code: request?.appleAuthorizationCode,
          app,
        });
        const body: AccountDeletion = {
          status: deletion.deletedInClerk ? "deleted" : "deleting",
          requestedAt: deletion.requestedAt.toISOString(),
          deletesAt: deletion.deletesAt.toISOString(),
        };
        return c.json(body, 202);
      },
    );
}
