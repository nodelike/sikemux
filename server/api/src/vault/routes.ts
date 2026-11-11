import type { Services } from "../app.ts";
import { Hono, type Context } from "hono";

import { requireIdentity, type AuthEnv } from "../auth.ts";
import { ApiFailure, badRequest, limitBody, readBody } from "../http.ts";
import { perUser } from "../limits.ts";
import {
  addKeys,
  addUses,
  createVault,
  putEntry,
  removeEntry,
  rotateVault,
} from "./change.ts";
import { vaultState, vaultUses } from "./state.ts";

const ENTRY_ID = /^[0-9a-f]{32}$/;
const REVISION = /^[1-9][0-9]{0,8}$/;

/** The most one sealed value takes in a body, with room for the fields around it. */
const SEALED_BYTES = 87_400 + 256;

function asker(c: Context<AuthEnv>) {
  const { userId, deviceKey } = c.get("identity");
  if (!deviceKey)
    throw new ApiFailure(403, "forbidden", "Only a host holds the vault.");
  return { userId, hostKey: deviceKey };
}

function entryId(c: Context): string {
  const id = c.req.param("id") ?? "";
  if (!ENTRY_ID.test(id))
    throw badRequest("A vault entry's id is 32 lowercase hex characters.");
  return id;
}

/**
 * The account's vault: sign-ins its hosts share, sealed under a key only they hold. The server
 * keeps the sealed entries and the key sealed for each host, and reads neither.
 */
export function vaultRoutes({
  database: { db },
  verifier,
  limiter,
}: Pick<Services, "database" | "verifier" | "limiter">) {
  const asHost = requireIdentity(verifier, db, { accept: ["host"] });
  const onAccount = requireIdentity(verifier, db, {
    accept: ["session", "host", "device", "mac"],
  });
  const state = (c: Context<AuthEnv>) => {
    const { userId, hostKey } = asker(c);
    return vaultState(db, userId, hostKey);
  };

  return new Hono<AuthEnv>()
    .get("/", asHost, perUser(limiter, "vault-get", 120), async (c) =>
      c.json(await state(c)),
    )
    .post(
      "/",
      limitBody<AuthEnv>(3 * SEALED_BYTES),
      asHost,
      perUser(limiter, "vault-create", 10),
      async (c) => {
        await createVault(db, asker(c), await readBody(c, "VaultCreate"));
        return c.json(await state(c), 201);
      },
    )
    .put(
      "/keys",
      limitBody<AuthEnv>(51 * SEALED_BYTES),
      asHost,
      perUser(limiter, "vault-keys", 30),
      async (c) => {
        await addKeys(db, asker(c), await readBody(c, "VaultKeysAdd"));
        return c.json(await state(c));
      },
    )
    .post(
      "/rotate",
      limitBody<AuthEnv>(552 * SEALED_BYTES),
      asHost,
      perUser(limiter, "vault-rotate", 10),
      async (c) => {
        await rotateVault(db, asker(c), await readBody(c, "VaultRotate"));
        return c.json(await state(c));
      },
    )
    .put(
      "/entries/:id",
      limitBody<AuthEnv>(2 * SEALED_BYTES),
      asHost,
      perUser(limiter, "vault-put", 120),
      async (c) => {
        const id = entryId(c);
        const body = await readBody(c, "VaultEntryPut");
        return c.json(await putEntry(db, asker(c), id, body));
      },
    )
    .delete(
      "/entries/:id",
      asHost,
      perUser(limiter, "vault-delete", 120),
      async (c) => {
        const id = entryId(c);
        const revision = c.req.query("revision") ?? "";
        if (!REVISION.test(revision))
          throw badRequest(
            "Send the entry's current revision as ?revision=<number>.",
          );
        await removeEntry(db, asker(c), id, Number(revision));
        return c.body(null, 204);
      },
    )
    .post(
      "/uses",
      limitBody<AuthEnv>(256 * 1024),
      asHost,
      perUser(limiter, "vault-uses-add", 60),
      async (c) => {
        await addUses(db, asker(c), await readBody(c, "VaultUsesAdd"));
        return c.body(null, 204);
      },
    )
    .get("/uses", onAccount, perUser(limiter, "vault-uses", 120), async (c) => {
      const entry = c.req.query("entry");
      if (entry !== undefined && !ENTRY_ID.test(entry))
        throw badRequest("A vault entry's id is 32 lowercase hex characters.");
      return c.json(await vaultUses(db, c.get("identity").userId, entry));
    });
}
