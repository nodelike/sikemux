import type { Services } from "../app.ts";
import type {
  Account,
  AccountDeletion,
  SignInsTidied,
} from "@sikemux/protocol";
import { Hono } from "hono";

import type { AuthEnv } from "../auth.ts";
import { isWeb, requireIdentity, requireRecentSignIn } from "../auth.ts";
import { conflict, notFound, readBody, readOptionalBody } from "../http.ts";
import { perUser } from "../limits.ts";
import { planOf } from "../billing/allowance.ts";
import { endDeletedBilling } from "../billing/deleted.ts";
import { revokeAppleSignIn } from "./apple.ts";
import { markDeleted, PURGE_AFTER_DAYS, restoreAccount } from "./deletion.ts";
import {
  addressRefusal,
  signInRefusalWords,
  stripWorkSignIns,
  takeRemovals,
} from "./sign-ins.ts";
import { isStaff } from "./staff.ts";

export function accountRoutes({
  database: { db },
  verifier,
  signInVerifier,
  limiter,
  clerk,
  appleSignIn: apple,
  push: { app },
  billing: { provider },
  addressProofKey,
}: Pick<
  Services,
  | "database"
  | "verifier"
  | "signInVerifier"
  | "limiter"
  | "clerk"
  | "appleSignIn"
  | "push"
  | "billing"
  | "addressProofKey"
>) {
  const admitted = requireIdentity(verifier, db, { allowDeleted: true });
  const anyAccount = requireIdentity(signInVerifier, db, {
    allowDeleted: true,
  });
  return new Hono<AuthEnv>()
    .use((c, next) =>
      c.req.method === "DELETE" || c.req.path.endsWith("/restore")
        ? anyAccount(c, next)
        : admitted(c, next),
    )
    .get("/", async (c) => {
      const { userId } = c.get("identity");
      const user = await db
        .selectFrom("users")
        .select(["deleted_at", "purge_after", "staff"])
        .where("id", "=", userId)
        .executeTakeFirst();
      const plan = await planOf(db, userId);
      const body: Account =
        user?.deleted_at && user.purge_after
          ? {
              status: "deleting",
              deletesAt: user.purge_after.toISOString(),
              restorableDays: PURGE_AFTER_DAYS,
              staff: user.staff,
              plan,
            }
          : {
              status: "active",
              restorableDays: PURGE_AFTER_DAYS,
              staff: user?.staff ?? false,
              plan,
            };
      return c.json(body);
    })
    .post("/restore", perUser(limiter, "restore-account", 10), async (c) => {
      const identity = c.get("identity");
      if (!(await restoreAccount(db, identity.userId)))
        throw notFound(
          "The account is already deleted and can no longer be restored.",
        );
      c.get("log").info("restored an account");
      const body: Account = {
        status: "active",
        restorableDays: PURGE_AFTER_DAYS,
        staff: await isStaff(db, identity.userId),
        plan: await planOf(db, identity.userId),
      };
      return c.json(body);
    })
    .post(
      "/sign-ins/check",
      perUser(limiter, "check-sign-in", 30),
      async (c) => {
        const { email } = await readBody(c, "SignInAddressCheck");
        const refusal = await addressRefusal(db, addressProofKey, email);
        if (refusal) throw conflict(signInRefusalWords(refusal));
        return c.body(null, 204);
      },
    )
    .post(
      "/sign-ins/tidy",
      perUser(limiter, "tidy-sign-ins", 30),
      async (c) => {
        const { userId } = c.get("identity");
        if (clerk)
          await stripWorkSignIns(
            db,
            clerk,
            addressProofKey,
            c.get("log"),
            userId,
          );
        const body: SignInsTidied = { removed: await takeRemovals(db, userId) };
        return c.json(body);
      },
    )
    .delete(
      "/",
      perUser(limiter, "delete-account", 10),
      requireRecentSignIn(),
      async (c) => {
        const identity = c.get("identity");
        const request = await readOptionalBody(c, "AccountDeletionRequest");
        const deletion = await markDeleted(db, {
          userId: identity.userId,
          actor: `user:${identity.userId}`,
          via: isWeb(identity) ? "web" : "phone",
          deletedInClerk: false,
        });
        const log = c.get("log");
        log.info({ at: deletion.requestedAt }, "deleted an account");
        await endDeletedBilling(db, provider, log, identity.userId);
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
