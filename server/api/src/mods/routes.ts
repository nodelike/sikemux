import type { Services } from "../app.ts";
import { Hono, type Context } from "hono";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import { ApiFailure, badRequest, limitBody, readBody } from "../http.ts";
import { limit, perUser } from "../limits.ts";
import { purgeMod, pushDocuments } from "./change.ts";
import { pullDocuments } from "./documents.ts";

const MOD_ID = /^[a-z0-9][a-z0-9-]{0,63}$/;
const SEQ = /^(0|[1-9][0-9]{0,15})$/;
const PUSH_BYTES = 4 * 1024 * 1024;

function asker(c: Context<AuthEnv>) {
  const { userId, deviceKey } = c.get("identity");
  if (!deviceKey)
    throw new ApiFailure(403, "forbidden", "Only a host keeps mod documents.");
  return { userId, hostKey: deviceKey };
}

/**
 * The account's record of every mod's documents, sealed under the vault key. Hosts keep their own
 * copy and merge through this one; the server sees mod ids, sizes and times, never contents.
 */
export function modRoutes({
  database: { db },
  verifier,
  limiter,
}: Pick<Services, "database" | "verifier" | "limiter">) {
  const asHost = requireIdentity(verifier, db, { accept: ["host"] });

  return new Hono<AuthEnv>()
    .post(
      "/documents",
      limitBody<AuthEnv>(PUSH_BYTES),
      asHost,
      limit<AuthEnv>(limiter, "mods-push", 30, (c) => asker(c).hostKey),
      async (c) => {
        const body = await readBody(c, "ModsPush");
        return c.json(await pushDocuments(db, asker(c), body));
      },
    )
    .get(
      "/documents",
      asHost,
      perUser(limiter, "mods-pull", 240),
      async (c) => {
        const since = c.req.query("since") ?? "";
        if (!SEQ.test(since))
          throw badRequest(
            "Send the last seq this host saw as ?since=<number>.",
          );
        return c.json(await pullDocuments(db, asker(c).userId, Number(since)));
      },
    )
    .delete(
      "/:mod/documents",
      asHost,
      perUser(limiter, "mods-purge", 30),
      async (c) => {
        const mod = c.req.param("mod");
        if (!MOD_ID.test(mod))
          throw badRequest(
            "A mod's id is lowercase letters, digits and dashes, not starting with a dash.",
          );
        return c.json(await purgeMod(db, asker(c), mod));
      },
    );
}
